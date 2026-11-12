//! Flight-recorder proof. Safe code only. A failed internal check panics
//! (status 35). Every tick is a literal. The record here is local: this
//! proof does not touch a live shell record, and nothing it builds is kept
//! past the QEMU exit.

use core::fmt::Write;

use crate::qemu_exit;
use crate::recorder::{self, AppendError, Recorder, Verify};
use crate::selftest;
use crate::serial;

const RES: usize = 2;

pub fn run_core() -> ! {
    let mut recorder = Recorder::<6>::new();

    let _ = write!(
        serial::Writer,
        "R3ATH3_RECORDER_CORE step=1 len=0 lost=0 head={:016x} verify=ok\n",
        recorder.head()
    );
    if recorder.len() != 0 || recorder.lost() != 0 || recorder.verify() != Verify::Ok {
        panic!("step 1");
    }

    let n = must_ok(recorder.append(10, b"human last=ok", b"help"), 2);
    let link = must_get(&recorder, n).link();
    let _ = write!(
        serial::Writer,
        "R3ATH3_RECORDER_CORE step=2 n={n} link={link:016x}\n"
    );

    let n = must_ok(
        recorder.append(20, b"agent ai1", b"say seq=1 done text=hi"),
        3,
    );
    if recorder.verify() != Verify::Ok {
        panic!("step 3");
    }
    let link = must_get(&recorder, n).link();
    let _ = write!(
        serial::Writer,
        "R3ATH3_RECORDER_CORE step=3 n={n} link={link:016x} verify=ok\n"
    );

    must_ok(recorder.append(30, b"gate", b"grant ai1 say"), 4);
    let before = yes_no(recorder.admits_agent(RES));
    must_ok(recorder.append(40, b"gate", b"revoke ai1 all"), 4);
    let after = yes_no(recorder.admits_agent(RES));
    let _ = write!(
        serial::Writer,
        "R3ATH3_RECORDER_CORE step=4 len={} agent_before={before} agent_after={after}\n",
        recorder.len()
    );

    must_ok(recorder.append(50, b"human last=cmd", b"approve 9"), 5);
    must_ok(recorder.append(60, b"selftest", b"ok=7 fail=0"), 5);
    if !recorder.is_full() {
        panic!("step 5 full");
    }
    let head_before = recorder.head();
    match recorder.append(70, b"human last=ok", b"log") {
        Err(AppendError::Full) => {}
        _ => panic!("step 5 append"),
    }
    if recorder.lost() != 1 || recorder.head() != head_before {
        panic!("step 5 lost");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_RECORDER_CORE step=5 len={} full=yes lost={} head={:016x}\n",
        recorder.len(),
        recorder.lost(),
        recorder.head()
    );

    let mut fresh = Recorder::<6>::new();
    let long_tag = [b'a'; 25];
    let long_body = [b'b'; 77];
    must_bad(fresh.append(1, &long_tag, b"x"), 6);
    must_bad(fresh.append(1, b"tag", &long_body), 6);
    must_bad(fresh.append(1, b"tag", b"tab\there"), 6);
    must_bad(fresh.append(1, b"", b"x"), 6);
    fresh.note_lost();
    if fresh.len() != 0 || fresh.lost() != 5 {
        panic!("step 6 totals");
    }
    serial::write_str("R3ATH3_RECORDER_CORE step=6 bad=4 noted=1 len=0 lost=5\n");

    recorder.corrupt_for_test(3);
    match recorder.verify() {
        Verify::Broken { at: 3 } => {}
        _ => panic!("step 7 verify"),
    }
    if recorder.verify_upto(2) != Verify::Ok {
        panic!("step 7 upto");
    }
    serial::write_str("R3ATH3_RECORDER_CORE step=7 tamper=3 verify=broken at=3 upto2=ok\n");

    if recorder.get(0).is_some() || recorder.get(7).is_some() {
        panic!("step 8 range");
    }
    let first = must_get(&recorder, 1);
    if first.tick() != 10 || first.tag() != b"human last=ok" || first.body() != b"help" {
        panic!("step 8 get");
    }
    serial::write_str(
        "R3ATH3_RECORDER_CORE step=8 get0=none get7=none get1=10,human last=ok,help\n",
    );

    let _ = write!(serial::Writer, "R3ATH3_RECORDER_CORE step=9 card=");
    selftest::write_check_line(&mut serial::Writer, "frames", Err("no free frame"));
    serial::write_str("\n");

    let _ = write!(serial::Writer, "R3ATH3_RECORDER_CORE step=10 card=");
    selftest::write_summary(&mut serial::Writer, 7, 1);
    serial::write_str("\n");

    let expired = recorder::event_body(
        Some(b"ai1"),
        "halt",
        Some(1),
        Some(4),
        "denied",
        None,
        Some("timeout"),
        None,
    );
    let failed = recorder::event_body(
        Some(b"ai1"),
        "spawn-spin",
        Some(7),
        Some(5),
        "failed",
        None,
        Some("full"),
        None,
    );
    if expired.overflow() || failed.overflow() {
        panic!("step 11 overflow");
    }
    serial::write_str("R3ATH3_RECORDER_CORE step=11 expired=");
    serial::write_bytes(expired.as_bytes());
    serial::write_str("|failed=");
    serial::write_bytes(failed.as_bytes());
    serial::write_str("\n");

    serial::write_str("R3ATH3_RECORDER_CORE ok steps=11\n");
    qemu_exit::recorder_core_ok();
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn must_ok(result: Result<u32, AppendError>, step: u32) -> u32 {
    match result {
        Ok(n) => n,
        Err(_) => panic!("step {step} append"),
    }
}

fn must_bad(result: Result<u32, AppendError>, step: u32) {
    match result {
        Err(AppendError::Bad) => {}
        _ => panic!("step {step} bad"),
    }
}

fn must_get(recorder: &Recorder<6>, n: u32) -> recorder::Entry {
    match recorder.get(n) {
        Some(entry) => entry,
        None => panic!("missing {n}"),
    }
}
