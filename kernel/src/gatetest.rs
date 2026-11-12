//! Gate proofs. Safe code only. A failed internal check panics (status 35).

use core::fmt::Write;

#[cfg(feature = "gate-core")]
use crate::gate::{self, BadLine, Refusal};
use crate::gate::{Action, Decision, Demo, Gate, Name, Perm, Request};
use crate::qemu_exit;
use crate::serial;

#[cfg(feature = "gate-core")]
pub fn run_core() -> ! {
    let mut gate = Gate::new(100);
    let mut now = 0u64;
    let ai1 = name(b"ai1");

    match gate.ask(request(ai1, 1, Action::Read), now) {
        Decision::Refused(Refusal::NoBadge) => step(1, "refused=no-badge"),
        _ => panic!("step 1"),
    }

    must_grant(&mut gate, ai1, Perm::Read);
    match gate.ask(request(ai1, 1, Action::Read), now) {
        Decision::Run(Action::Read) => step(2, "run=read"),
        _ => panic!("step 2"),
    }

    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Refused(Refusal::NoPermission(Perm::Spawn)) => step(3, "refused=no-permission"),
        _ => panic!("step 3"),
    }

    must_grant(&mut gate, ai1, Perm::Spawn);
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Pending { rq: 1 } => step(4, "pending rq=1"),
        _ => panic!("step 4"),
    }

    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Spin)), now) {
        Decision::Refused(Refusal::Busy) => step(5, "refused=busy"),
        _ => panic!("step 5"),
    }

    if gate.approve(2).is_ok() {
        panic!("step 6 approve 2");
    }
    match gate.approve(1) {
        Ok(gate::Approved::Go(pending))
            if pending.rq == 1 && pending.action == Action::Spawn(Demo::Nap) =>
        {
            step(6, "wrong=no-pending approved rq=1 action=spawn-nap")
        }
        _ => panic!("step 6"),
    }

    if gate.approve(1).is_ok() {
        panic!("step 7 replay");
    }
    step(7, "replay=no-pending");

    now = 10;
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Pending { rq: 2 } => {}
        _ => panic!("step 8 ask"),
    }
    match gate.deny(2) {
        Ok(pending) if pending.rq == 2 => step(8, "denied rq=2"),
        _ => panic!("step 8 deny"),
    }

    now = 20;
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Pending { rq: 3 } => {}
        _ => panic!("step 9 ask"),
    }
    match gate.revoke(&ai1, Some(Perm::Spawn)) {
        Ok(Some(pending)) if pending.rq == 3 => {}
        _ => panic!("step 9 revoke"),
    }
    if gate.approve(3).is_ok() {
        panic!("step 9 approve");
    }
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Refused(Refusal::NoPermission(Perm::Spawn)) => {
            step(9, "cancelled rq=3 then=no-pending next=no-permission")
        }
        _ => panic!("step 9 next"),
    }

    match gate.ask(request(ai1, 1, Action::Read), now) {
        Decision::Run(Action::Read) => step(10, "run=read"),
        _ => panic!("step 10"),
    }

    must_grant(&mut gate, ai1, Perm::Halt);
    now = 30;
    match gate.ask(request(ai1, 1, Action::Halt), now) {
        Decision::Pending { rq: 4 } => {}
        _ => panic!("step 11 ask"),
    }
    if gate.poll(129).is_some() {
        panic!("step 11 early");
    }
    match gate.poll(130) {
        Some(pending) if pending.rq == 4 => {}
        _ => panic!("step 11 poll"),
    }
    if gate.approve(4).is_ok() {
        panic!("step 11 approve");
    }
    step(11, "expired rq=4 at=130");

    must_grant(&mut gate, ai1, Perm::Spawn);
    now = 200;
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), now) {
        Decision::Pending { rq: 5 } => {}
        _ => panic!("step 12 ask"),
    }
    gate.force_clear_for_test(&ai1, Perm::Spawn);
    match gate.approve(5) {
        Ok(gate::Approved::Cancelled(pending)) if pending.rq == 5 => {
            step(12, "recheck=cancelled rq=5")
        }
        _ => panic!("step 12"),
    }

    match gate.revoke(&ai1, None) {
        Ok(_) => {}
        Err(_) => panic!("step 13 revoke"),
    }
    match gate.ask(request(ai1, 1, Action::Read), now) {
        Decision::Refused(Refusal::NoBadge) => {}
        _ => panic!("step 13 read"),
    }
    for byte in [b"a", b"b", b"c", b"d"] {
        must_grant(&mut gate, name(byte), Perm::Read);
    }
    match gate.grant(name(b"e"), Perm::Read) {
        Err(gate::GrantError::BadgeTableFull) => step(13, "removed refused=no-badge full=e"),
        _ => panic!("step 13 full"),
    }

    let mut long = [0u8; 81];
    let prefix = b"R3GATE ASK ai1 2 say ";
    long[..prefix.len()].copy_from_slice(prefix);
    for byte in &mut long[prefix.len()..] {
        *byte = b'x';
    }
    let lines: [&[u8]; 7] = [
        b"R3GATE ASK ai1 1 read",
        b"halt",
        &long,
        b"R3GATE ASK ai1 3 approve 1",
        b"R3GATE ASK ai1 4 spawn bomb",
        b"R3GATE ASK AI1 5 read",
        b"R3GATE ASK ai1 1 read ",
    ];
    let mut words = [""; 7];
    for (index, line) in lines.iter().enumerate() {
        words[index] = match gate::parse_request(line) {
            Ok(_) => "ok",
            Err((bad, _)) => bad.as_str(),
        };
    }
    expect_parse(gate::parse_request(lines[0]), true, None);
    expect_parse(
        gate::parse_request(lines[1]),
        false,
        Some((BadLine::Syntax, false)),
    );
    expect_parse(
        gate::parse_request(lines[2]),
        false,
        Some((BadLine::TooLong, false)),
    );
    expect_parse(
        gate::parse_request(lines[3]),
        false,
        Some((BadLine::Verb, true)),
    );
    expect_parse(
        gate::parse_request(lines[4]),
        false,
        Some((BadLine::Arg, true)),
    );
    expect_parse(
        gate::parse_request(lines[5]),
        false,
        Some((BadLine::Syntax, false)),
    );
    expect_parse(
        gate::parse_request(lines[6]),
        false,
        Some((BadLine::Syntax, true)),
    );
    let _ = write!(
        serial::Writer,
        "R3ATH3_GATE_CORE step=14 parse={},{},{},{},{},{},{}\n",
        words[0],
        words[1],
        words[2],
        words[3],
        words[4],
        words[5],
        words[6]
    );

    serial::write_str("R3ATH3_GATE_CORE ok steps=14\n");
    qemu_exit::gate_core_ok();
}

#[cfg(feature = "gate-expire")]
pub fn run_expire() -> ! {
    use crate::irq;
    use crate::sched;

    let mut gate = Gate::new(50);
    let ai1 = name(b"ai1");
    must_grant(&mut gate, ai1, Perm::Spawn);
    let t0 = irq::ticks();
    match gate.ask(request(ai1, 1, Action::Spawn(Demo::Nap)), t0) {
        Decision::Pending { rq: 1 } => {}
        _ => panic!("expire ask"),
    }
    let mut early_polls: u64 = 0;
    loop {
        sched::sleep_ticks(5);
        let now = irq::ticks();
        if let Some(pending) = gate.poll(now) {
            if pending.rq != 1 {
                panic!("expire rq={}", pending.rq);
            }
            let waited = now.wrapping_sub(t0);
            irq::without_interrupts(|| {
                let _ = write!(
                    serial::Writer,
                    "R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited={waited} early_polls={early_polls}\n"
                );
            });
            qemu_exit::gate_expire_ok();
        }
        early_polls = early_polls.wrapping_add(1);
    }
}

fn name(bytes: &[u8]) -> Name {
    Name::parse(bytes).unwrap_or_else(|| panic!("bad name"))
}

fn request(agent: Name, seq: u32, action: Action) -> Request {
    Request { agent, seq, action }
}

fn must_grant(gate: &mut Gate, agent: Name, perm: Perm) {
    if gate.grant(agent, perm).is_err() {
        panic!("grant failed");
    }
}

#[cfg(feature = "gate-core")]
fn step(n: u32, text: &str) {
    let _ = write!(serial::Writer, "R3ATH3_GATE_CORE step={n} {text}\n");
}

#[cfg(feature = "gate-core")]
fn expect_parse(
    result: Result<Request, (BadLine, Option<(Name, u32)>)>,
    ok: bool,
    bad: Option<(BadLine, bool)>,
) {
    match (ok, bad, result) {
        (true, None, Ok(_)) => {}
        (false, Some((want, true)), Err((got, Some(_)))) if got == want => {}
        (false, Some((want, false)), Err((got, None))) if got == want => {}
        _ => panic!("parse shape"),
    }
}
