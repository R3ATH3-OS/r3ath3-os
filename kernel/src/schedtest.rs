//! Scheduler proofs. Safe code only. Every judged line is written with IF clear
//! so a preemption cannot split it. A failed internal check panics (status 35).

use core::fmt::Write;
#[cfg(any(
    feature = "sched-preempt",
    feature = "sched-sleep",
    feature = "sched-exit"
))]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "sched-exit")]
use crate::heap;
use crate::irq;
use crate::qemu_exit;
use crate::sched;
#[cfg(feature = "sched-full")]
use crate::sched::SpawnError;
use crate::serial;

fn emit(args: core::fmt::Arguments<'_>) {
    irq::without_interrupts(|| {
        let _ = serial::Writer.write_fmt(args);
    });
}

#[cfg(feature = "sched-coop")]
pub fn run_coop() -> ! {
    fn task(arg: usize) {
        let name = match arg {
            0 => "A",
            1 => "B",
            _ => "C",
        };
        for step in 1..=3 {
            emit(format_args!("R3ATH3_SCHED_COOP task={name} step={step}\n"));
            sched::yield_now();
        }
    }
    must_spawn("A", task, 0);
    must_spawn("B", task, 1);
    must_spawn("C", task, 2);
    must_wait(2);
    must_wait(3);
    must_wait(4);
    let switches = sched::switches();
    emit(format_args!(
        "R3ATH3_SCHED_COOP ok order=ABCABCABC switches={switches}\n"
    ));
    qemu_exit::sched_coop_ok();
}

#[cfg(feature = "sched-preempt")]
pub fn run_preempt() -> ! {
    static HOG: AtomicU64 = AtomicU64::new(0);

    fn hog(_: usize) {
        loop {
            HOG.fetch_add(1, Ordering::Relaxed);
            core::hint::spin_loop();
        }
    }

    fn worker(_: usize) {
        for step in 1..=5 {
            emit(format_args!("R3ATH3_SCHED_PREEMPT worker step={step}\n"));
            sched::sleep_ticks(1);
        }
    }

    must_spawn("hog", hog, 0);
    must_spawn("worker", worker, 0);
    must_wait(3);
    let hog_spins = HOG.load(Ordering::Relaxed);
    let preemptions = sched::preemptions();
    emit(format_args!(
        "R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins={hog_spins} preemptions={preemptions}\n"
    ));
    qemu_exit::sched_preempt_ok();
}

#[cfg(feature = "sched-sleep")]
pub fn run_sleep() -> ! {
    static START: AtomicU64 = AtomicU64::new(0);

    fn sleeper(arg: usize) {
        let (name, want) = match arg {
            0 => ("L", 30u64),
            1 => ("M", 20u64),
            _ => ("S", 10u64),
        };
        let start = START.load(Ordering::Relaxed);
        sched::sleep_until(start.wrapping_add(want));
        let elapsed = irq::ticks().wrapping_sub(start);
        emit(format_args!(
            "R3ATH3_SCHED_SLEEP name={name} want={want} elapsed={elapsed}\n"
        ));
    }

    let start = irq::ticks();
    START.store(start, Ordering::Relaxed);
    must_spawn("L", sleeper, 0);
    must_spawn("M", sleeper, 1);
    must_spawn("S", sleeper, 2);
    must_wait(2);
    must_wait(3);
    must_wait(4);
    let idle_runs = sched::idle_runs();
    emit(format_args!(
        "R3ATH3_SCHED_SLEEP ok count=3 idle_runs={idle_runs}\n"
    ));
    qemu_exit::sched_sleep_ok();
}

#[cfg(feature = "sched-full")]
pub fn run_full() -> ! {
    fn noop(_: usize) {}

    for _ in 0..6 {
        must_spawn("task", noop, 0);
    }
    match sched::spawn("task", noop, 0) {
        Err(SpawnError::Full) => {}
        Ok(id) => panic!("task table accepted id={id}"),
    }
    for id in 2..=7 {
        must_wait(id);
    }
    let id = must_spawn("task", noop, 0);
    if id != 2 {
        panic!("after reap id={id}");
    }
    must_wait(id);
    emit(format_args!(
        "R3ATH3_SCHED_FULL ok spawned=6 error=full after_reap=2\n"
    ));
    qemu_exit::sched_full_ok();
}

#[cfg(feature = "sched-reap")]
pub fn run_reap() -> ! {
    use core::sync::atomic::{AtomicU8, Ordering};

    static W1_REAPED: AtomicU8 = AtomicU8::new(0);
    static W2_REAPED: AtomicU8 = AtomicU8::new(0);

    fn task_t(_: usize) {
        sched::yield_now();
    }

    fn task_w1(_: usize) {
        match sched::wait(2) {
            Ok(true) => W1_REAPED.store(1, Ordering::Relaxed),
            Ok(false) => W1_REAPED.store(2, Ordering::Relaxed),
            Err(_) => panic!("w1 wait"),
        }
    }

    fn task_w2(_: usize) {
        match sched::wait(2) {
            Ok(true) => W2_REAPED.store(1, Ordering::Relaxed),
            Ok(false) => W2_REAPED.store(2, Ordering::Relaxed),
            Err(_) => panic!("w2 wait"),
        }
    }

    must_spawn("T", task_t, 0);
    must_spawn("W1", task_w1, 0);
    must_spawn("W2", task_w2, 0);
    must_wait(3);
    must_wait(4);
    let w1 = W1_REAPED.load(Ordering::Relaxed);
    let w2 = W2_REAPED.load(Ordering::Relaxed);
    let free = sched::free_slots();
    if w1 != 1 || w2 != 2 || free != 6 {
        panic!("sched-reap w1={w1} w2={w2} free={free}");
    }
    emit(format_args!(
        "R3ATH3_SCHED_REAP waiters=2 reaped_by=W1 w2_reaped=no free=6\n"
    ));
    sched::selftest_stale_reap();
    emit(format_args!(
        "R3ATH3_SCHED_REAP stale_gen=kept same_gen=reaped\n"
    ));
    emit(format_args!("R3ATH3_SCHED_REAP ok\n"));
    qemu_exit::sched_reap_ok();
}

#[cfg(feature = "sched-exit")]
pub fn run_exit() -> ! {
    static RAN: AtomicU64 = AtomicU64::new(0);

    fn runner(_: usize) {
        RAN.fetch_add(1, Ordering::Relaxed);
    }

    let before = heap::used();
    for _ in 0..2 {
        for _ in 0..6 {
            must_spawn("task", runner, 0);
        }
        for id in 2..=7 {
            must_wait(id);
        }
    }
    let free = sched::free_slots();
    let ran = RAN.load(Ordering::Relaxed);
    let heap_delta = heap::used().wrapping_sub(before);
    if free != 6 || ran != 12 || heap_delta != 0 {
        panic!("sched-exit free={free} ran={ran} heap_delta={heap_delta}");
    }
    emit(format_args!(
        "R3ATH3_SCHED_EXIT ok rounds=2 spawned=12 ran=12 free=6 heap_delta=0\n"
    ));
    qemu_exit::sched_exit_ok();
}

fn must_spawn(name: &'static str, entry: fn(usize), arg: usize) -> usize {
    match sched::spawn(name, entry, arg) {
        Ok(id) => id,
        Err(_) => panic!("spawn failed"),
    }
}

fn must_wait(id: usize) {
    if sched::wait(id).is_err() {
        panic!("wait failed id={id}");
    }
}
