//! `glue-tone` proof. Glue Duty's frequency word on PIT channel 2.
//! The latch is the proof. Nothing here captures audio. Safe code only.

use core::fmt::Write;

use crate::apps::{self, GLUE_DUTY, HELLO};
use crate::frames;
use crate::heap;
use crate::irq;
use crate::qemu_exit;
use crate::serial;
use crate::speaker::{self, Played};
use crate::wasm;

const SCRUB_STEP: u32 = 34;
const LATCH_SLACK: u16 = 20;

pub fn run() -> ! {
    let heap_before = heap::used();
    let free_before = frames::stats().1;
    let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE start\n");

    let ran = wasm::with_instance(GLUE_DUTY, |loaded| {
        let mut inst = match loaded {
            Ok(inst) => inst,
            Err(error) => fail(0, error.word()),
        };
        if !apps::check_abi(&mut inst) {
            fail(0, "abi");
        }
        match inst.call("r3_init", &[1]) {
            Ok(_) => {}
            Err(trap) => fail(0, trap.word()),
        }
        play_expect(&mut inst, 1, 250, 4772);
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_TONE step=1 hz=250 divisor=4772 latched=ok gate=3\n"
        );

        key_step(&mut inst, b' ', 1);
        play_expect(&mut inst, 2, 400, 2982);
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_TONE step=2 hz=400 divisor=2982 latched=ok gate=3\n"
        );

        let ticks = irq::wait_ticks(5);
        if ticks != 5 {
            fail(3, "ticks");
        }
        let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE step=3 ticks=5\n");

        key_step(&mut inst, b'p', 2);
        play_expect(&mut inst, 4, 0, 0);
        let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE step=4 hz=0 gate=0\n");

        key_step(&mut inst, b'p', 1);
        play_expect(&mut inst, 5, 400, 2982);
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_TONE step=5 hz=400 divisor=2982 latched=ok gate=3\n"
        );
    });
    if ran.is_none() {
        fail(0, "busy");
    }

    let ran = wasm::with_instance(GLUE_DUTY, |loaded| {
        let mut inst = match loaded {
            Ok(inst) => inst,
            Err(error) => fail(6, error.word()),
        };
        match inst.call("r3_init", &[1]) {
            Ok(_) => {}
            Err(trap) => fail(6, trap.word()),
        }
        let mut n = 1u32;
        while n <= SCRUB_STEP {
            let mut keys = [0u8; 4];
            let len = proof_keys(n, &mut keys);
            let mut i = 0;
            while i < len {
                match inst.call("r3_key", &[i64::from(keys[i])]) {
                    Ok(_) => {}
                    Err(trap) => fail(6, trap.word()),
                }
                i += 1;
            }
            match inst.call("r3_step", &[]) {
                Ok(Some(_)) => {}
                Ok(None) => fail(6, "step-none"),
                Err(trap) => fail(6, trap.word()),
            }
            let hz = match apps::exported_tone(&mut inst) {
                Ok(Some(hz)) => hz,
                Ok(None) => fail(6, "no-tone"),
                Err(trap) => fail(6, trap.word()),
            };
            if n < SCRUB_STEP && hz == 500 {
                fail(6, "early");
            }
            if n == SCRUB_STEP && hz != 500 {
                fail(6, "scrub");
            }
            n += 1;
        }
        play_expect(&mut inst, 6, 500, 2386);
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_TONE step=6 scrub=34 hz=500 divisor=2386 latched=ok gate=3\n"
        );
    });
    if ran.is_none() {
        fail(6, "busy");
    }

    let off = speaker::tone(0);
    if off.hz != 0 || off.divisor != 0 || off.gate != 0 {
        fail_play(7, &off);
    }
    let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE step=7 off hz=0 gate=0\n");

    let ran = wasm::with_instance(HELLO, |loaded| {
        let mut inst = match loaded {
            Ok(inst) => inst,
            Err(error) => fail(8, error.word()),
        };
        match apps::exported_tone(&mut inst) {
            Ok(None) => {}
            Ok(Some(_)) => fail(8, "export"),
            Err(trap) => fail(8, trap.word()),
        }
    });
    if ran.is_none() {
        fail(8, "busy");
    }
    let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE step=8 absent=ok\n");

    if heap::used() != heap_before || frames::stats().1 != free_before {
        fail(9, "changed");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_TONE step=9 heap=same frames=same\n"
    );
    let _ = write!(serial::Writer, "R3ATH3_GLUE_TONE ok\n");
    qemu_exit::glue_tone_ok();
}

fn key_step(inst: &mut wasm::Instance<'_, '_>, key: u8, state: i64) {
    match inst.call("r3_key", &[i64::from(key)]) {
        Ok(_) => {}
        Err(trap) => fail(0, trap.word()),
    }
    match inst.call("r3_step", &[]) {
        Ok(Some(got)) if got == state => {}
        Ok(Some(_)) => fail(0, "state"),
        Ok(None) => fail(0, "step-none"),
        Err(trap) => fail(0, trap.word()),
    }
}

fn play_expect(inst: &mut wasm::Instance<'_, '_>, step: u8, hz: u32, divisor: u16) {
    let got = match apps::exported_tone(inst) {
        Ok(Some(hz)) => hz,
        Ok(None) => fail(step, "no-tone"),
        Err(trap) => fail(step, trap.word()),
    };
    let played = speaker::tone(got);
    if got != hz || played.hz != hz || played.divisor != divisor || !gate_ok(hz, &played) {
        fail_play(step, &played);
    }
}

fn gate_ok(hz: u32, played: &Played) -> bool {
    if hz == 0 {
        played.gate == 0 && played.latched == 0
    } else {
        played.gate == 3 && latch_ok(played.divisor, played.latched)
    }
}

/// Even, and in `divisor - 20 ..= divisor`. The reload value itself counts:
/// reprogramming a running channel 2 can latch the new divisor before the
/// first PIT clock. `speaker-core` uses a half-open window; this proof
/// includes the endpoint this QEMU returns.
fn latch_ok(divisor: u16, latched: u16) -> bool {
    latched != 0 && latched <= divisor && divisor - latched <= LATCH_SLACK && latched % 2 == 0
}

const PROOF_KEYS: &[u8] = include_bytes!("../../apps/glue-duty/proof-keys.bin");

fn proof_keys(n: u32, out: &mut [u8; 4]) -> usize {
    if PROOF_KEYS.len() != 2000 || n == 0 || n as usize > PROOF_KEYS.len() {
        fail(6, "keys");
    }
    let keys: &[u8] = match PROOF_KEYS[(n - 1) as usize] {
        0 => b"",
        1 => b" ",
        2 => b"a",
        3 => b"d",
        4 => b"wa",
        5 => b"wd",
        6 => b"sa",
        7 => b"sd",
        8 => b"w",
        9 => b"s",
        _ => fail(6, "keys"),
    };
    let mut i = 0;
    while i < keys.len() {
        out[i] = keys[i];
        i += 1;
    }
    keys.len()
}

fn fail(step: u8, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_TONE fail step={step} detail={detail}\n"
    );
    qemu_exit::fail();
}

fn fail_play(step: u8, played: &Played) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_TONE fail step={step} detail=play hz={} divisor={} latched={} gate={}\n",
        played.hz, played.divisor, played.latched, played.gate
    );
    qemu_exit::fail();
}
