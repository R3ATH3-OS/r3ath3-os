//! On-machine checks and the report card. See `docs/RECORDER.md`.
//!
//! Safe code only. No statics and no allocation. This module never sees the
//! live flight recorder or the live gate. The shell passes `live_ok` and the
//! store result. The eight checks run only in the shell build. `recorder-core`
//! uses the card formatters. A normal check never panics.
#![allow(dead_code)]

use core::fmt::Write;

#[cfg(feature = "shell")]
use crate::frames;
#[cfg(feature = "shell")]
use crate::gate::{Action, Approved, Decision, Demo, Gate, Name, Perm, Refusal, Request};
#[cfg(feature = "shell")]
use crate::heap;
#[cfg(feature = "shell")]
use crate::irq;
#[cfg(feature = "shell")]
use crate::recorder::{AppendError, Recorder, Verify};
#[cfg(feature = "shell")]
use crate::sched::{self, State};
#[cfg(feature = "shell")]
use crate::script::{self, Sink};

pub const CHECKS: usize = 8;
pub const NAMES: [&str; CHECKS] = [
    "timer", "sched", "frames", "script", "gate", "recorder", "store", "heap",
];

#[cfg(feature = "shell")]
const TIMER_SPIN_LIMIT: u32 = 2_000_000;

#[derive(Clone, Copy)]
pub struct Card {
    results: [Result<(), &'static str>; CHECKS],
}

impl Card {
    pub fn ok_count(&self) -> usize {
        self.results.iter().filter(|result| result.is_ok()).count()
    }

    pub fn fail_count(&self) -> usize {
        CHECKS - self.ok_count()
    }
}

pub fn write_header(out: &mut impl Write) {
    let _ = write!(out, "selftest: {CHECKS} checks");
}

pub fn write_check_line(out: &mut impl Write, name: &str, result: Result<(), &str>) {
    let _ = write!(out, "selftest {name:<8} ");
    match result {
        Ok(()) => {
            let _ = write!(out, "ok");
        }
        Err(detail) => {
            let _ = write!(out, "FAIL {detail}");
        }
    }
}

pub fn write_summary(out: &mut impl Write, ok: usize, fail: usize) {
    if fail == 0 {
        let _ = write!(out, "selftest: {ok} of {CHECKS} ok");
    } else {
        let _ = write!(out, "selftest: {ok} of {CHECKS} ok, {fail} failed");
    }
}

pub fn write_card(out: &mut impl Write, card: &Card) {
    write_header(out);
    let _ = write!(out, "\n");
    for (name, result) in NAMES.iter().zip(card.results.iter()) {
        write_check_line(out, name, *result);
        let _ = write!(out, "\n");
    }
    write_summary(out, card.ok_count(), card.fail_count());
    let _ = write!(out, "\n");
}

#[cfg(feature = "shell")]
pub fn run(heap_before: usize, live_ok: bool, store: Result<(), &'static str>) -> Card {
    let timer = check_timer();
    let sched = check_sched(timer.is_ok());
    let frames = check_frames();
    let script = check_script();
    let gate = check_gate();
    let recorder = check_recorder(live_ok);
    let heap = check_heap(heap_before);
    Card {
        results: [timer, sched, frames, script, gate, recorder, store, heap],
    }
}

#[cfg(feature = "shell")]
fn check_timer() -> Result<(), &'static str> {
    let t0 = irq::ticks();
    let mut spins = 0u32;
    while spins < TIMER_SPIN_LIMIT {
        if irq::ticks() >= t0.saturating_add(5) {
            return Ok(());
        }
        sched::yield_now();
        spins += 1;
    }
    Err("ticks did not advance")
}

#[cfg(feature = "shell")]
fn check_sched(timer_ok: bool) -> Result<(), &'static str> {
    if !timer_ok {
        return Err("needs the timer");
    }
    let before = sched::switches();
    sched::sleep_ticks(2);
    if sched::switches() <= before {
        return Err("no task switch while sleeping");
    }
    let snap = sched::snapshot();
    match (snap[0], snap[1]) {
        (Some(shell), Some(idle))
            if shell.name == "shell"
                && shell.state == State::Running
                && idle.name == "idle" =>
        {
            Ok(())
        }
        _ => Err("task table does not show shell and idle"),
    }
}

#[cfg(feature = "shell")]
fn check_frames() -> Result<(), &'static str> {
    let (total, free) = frames::stats();
    if free == 0 {
        return Err("no free frame");
    }
    let Some(frame) = frames::alloc() else {
        return Err("no free frame");
    };
    let dropped = frames::stats().1 == free - 1;
    let freed = frames::free(frame).is_ok();
    let (total_after, free_after) = frames::stats();
    let restored = total_after == total && free_after == free;
    if !dropped {
        return Err("free count did not drop by one");
    }
    if !freed {
        return Err("free returned an error");
    }
    if !restored {
        return Err("counts not restored");
    }
    Ok(())
}

#[cfg(feature = "shell")]
struct Capture {
    lines: [[u8; 16]; 4],
    lens: [u8; 4],
    count: u8,
    bad: bool,
}

#[cfg(feature = "shell")]
impl Sink for Capture {
    fn line(&mut self, text: &[u8]) {
        if self.bad || self.count >= 4 || text.len() > 16 {
            self.bad = true;
            return;
        }
        let index = self.count as usize;
        self.lines[index][..text.len()].copy_from_slice(text);
        self.lens[index] = text.len() as u8;
        self.count += 1;
    }
}

#[cfg(feature = "shell")]
fn check_script() -> Result<(), &'static str> {
    let mut cap = Capture {
        lines: [[0; 16]; 4],
        lens: [0; 4],
        count: 0,
        bad: false,
    };
    if script::run_inline(b"set n 2; add n 3; print n; say \"ok\"", &mut cap).is_err() {
        return Err("script error");
    }
    if cap.bad || cap.count != 2 {
        return Err("script output differs");
    }
    let first = &cap.lines[0][..cap.lens[0] as usize];
    let second = &cap.lines[1][..cap.lens[1] as usize];
    if first == b"n=5" && second == b"ok" {
        Ok(())
    } else {
        Err("script output differs")
    }
}

#[cfg(feature = "shell")]
fn check_gate() -> Result<(), &'static str> {
    let mut gate = Gate::new(100);
    let Some(agent) = Name::parse(b"st") else {
        return Err("gate decision differs");
    };
    let ask = |gate: &mut Gate, seq, action, now| gate.ask(Request { agent, seq, action }, now);
    match ask(&mut gate, 1, Action::Read, 0) {
        Decision::Refused(Refusal::NoBadge) => {}
        _ => return Err("gate decision differs"),
    }
    if gate.grant(agent, Perm::Read).is_err() {
        return Err("gate decision differs");
    }
    match ask(&mut gate, 1, Action::Read, 0) {
        Decision::Run(Action::Read) => {}
        _ => return Err("gate decision differs"),
    }
    match ask(&mut gate, 1, Action::Spawn(Demo::Nap), 0) {
        Decision::Refused(Refusal::NoPermission(Perm::Spawn)) => {}
        _ => return Err("gate decision differs"),
    }
    if gate.grant(agent, Perm::Spawn).is_err() {
        return Err("gate decision differs");
    }
    match ask(&mut gate, 1, Action::Spawn(Demo::Nap), 0) {
        Decision::Pending { rq: 1 } => {}
        _ => return Err("gate decision differs"),
    }
    match ask(&mut gate, 1, Action::Spawn(Demo::Spin), 0) {
        Decision::Refused(Refusal::Busy) => {}
        _ => return Err("gate decision differs"),
    }
    if gate.approve(2).is_ok() {
        return Err("gate decision differs");
    }
    match gate.approve(1) {
        Ok(Approved::Go(pending)) if pending.rq == 1 => {}
        _ => return Err("gate decision differs"),
    }
    if gate.approve(1).is_ok() {
        return Err("gate decision differs");
    }
    match ask(&mut gate, 1, Action::Spawn(Demo::Nap), 10) {
        Decision::Pending { rq: 2 } => {}
        _ => return Err("gate decision differs"),
    }
    if gate.poll(109).is_some() {
        return Err("gate decision differs");
    }
    match gate.poll(110) {
        Some(pending) if pending.rq == 2 => Ok(()),
        _ => Err("gate decision differs"),
    }
}

#[cfg(feature = "shell")]
fn check_recorder(live_ok: bool) -> Result<(), &'static str> {
    if !live_ok {
        return Err("live chain broken");
    }
    let mut scratch = Recorder::<4>::new();
    for n in 1..=4u32 {
        match scratch.append(u64::from(n), b"selftest", b"x") {
            Ok(got) if got == n => {}
            _ => return Err("scratch log misbehaved"),
        }
    }
    if scratch.append(5, b"selftest", b"x") != Err(AppendError::Full) {
        return Err("scratch log misbehaved");
    }
    if scratch.lost() != 1 || scratch.verify() != Verify::Ok {
        return Err("scratch log misbehaved");
    }
    match scratch.get(4) {
        Some(entry) if entry.link() == scratch.head() => Ok(()),
        _ => Err("scratch log misbehaved"),
    }
}

#[cfg(feature = "shell")]
fn check_heap(heap_before: usize) -> Result<(), &'static str> {
    let now = heap::used();
    if now == heap_before && now <= heap::SIZE {
        Ok(())
    } else {
        Err("heap use changed during selftest")
    }
}
