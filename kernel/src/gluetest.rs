//! `wasm-glue` proof. The baked Glue Duty module and the native replay run
//! the same key script. Safe code only. A failed check prints one line and
//! exits through `fail`.

use core::fmt::Write;

use crate::apps::GLUE_DUTY;
use crate::qemu_exit;
use crate::serial;
use crate::wasm::{self, Trap};

const STEPS: u32 = 2000;
const STEP_FUEL: u32 = 250_000;

pub fn run() -> ! {
    let _ = write!(serial::Writer, "R3ATH3_WASM_GLUE start seed=1\n");
    let ran = wasm::with_instance(GLUE_DUTY, |loaded| {
        let mut inst = match loaded {
            Ok(inst) => inst,
            Err(error) => fail(0, error.word()),
        };
        if !crate::apps::check_abi(&mut inst) {
            fail(0, "abi");
        }
        match inst.call("r3_init", &[1]) {
            Ok(_) => {}
            Err(trap) => fail(0, trap.word()),
        }
        let mut prev = 0i64;
        let mut max_fuel = 0u32;
        for n in 1..=STEPS {
            let mut keys = [0u8; 4];
            let len = proof_keys(n, &mut keys);
            for i in 0..len {
                match inst.call("r3_key", &[i64::from(keys[i])]) {
                    Ok(_) => {}
                    Err(trap) => fail(n, trap.word()),
                }
            }
            let (state, fuel) = inst.call_metered("r3_step", &[], wasm::FUEL_PER_CALL);
            let state = match state {
                Ok(Some(value)) => value,
                Ok(None) => fail(n, "step-none"),
                Err(trap) => fail(n, trap.word()),
            };
            if fuel > max_fuel {
                max_fuel = fuel;
            }
            if fuel > STEP_FUEL {
                fail(n, "fuel-budget");
            }
            let frame = match grid_fnv(&mut inst) {
                Ok(frame) => frame,
                Err(trap) => fail(n, trap.word()),
            };
            let (summary, summary_len) = match crate::apps::read_summary(&mut inst) {
                Ok(text) => text,
                Err(trap) => fail(n, trap.word()),
            };
            if state != prev || n % 100 == 0 {
                let _ = write!(
                    serial::Writer,
                    "R3ATH3_WASM_GLUE step={n} state={state} frame={frame:016x} "
                );
                let mut filtered = [0u8; 60];
                for i in 0..summary_len {
                    filtered[i] = crate::apps::printable(summary[i]);
                }
                serial::write_bytes(&filtered[..summary_len]);
                serial::write_str("\n");
                prev = state;
            }
        }
        let _ = write!(serial::Writer, "R3ATH3_WASM_GLUE ok steps=2000\n");
    });
    if ran.is_none() {
        fail(0, "busy");
    }
    qemu_exit::wasm_glue_ok();
}

fn grid_fnv(inst: &mut wasm::Instance<'_, '_>) -> Result<u64, Trap> {
    let offset = match inst.call("r3_text", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return Err(Trap::Memory),
        Err(trap) => return Err(trap),
    };
    let memory = inst.memory();
    let Some(start) = crate::apps::offset_of(offset, memory.len(), 2000) else {
        return Err(Trap::Memory);
    };
    Ok(fnv64(&memory[start..start + 2000]))
}

fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// One packed key code per step. The same file drives `replay.rs`.
const PROOF_KEYS: &[u8] = include_bytes!("../../apps/glue-duty/proof-keys.bin");

/// Keys applied before step `n`. Codes: 0 none, 1 space, 2 a, 3 d,
/// 4 w+a, 5 w+d, 6 s+a, 7 s+d, 8 w, 9 s.
fn proof_keys(n: u32, out: &mut [u8; 4]) -> usize {
    if PROOF_KEYS.len() != STEPS as usize || n == 0 || n as usize > PROOF_KEYS.len() {
        fail(n, "keys");
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
        _ => fail(n, "keys"),
    };
    let mut i = 0;
    while i < keys.len() {
        out[i] = keys[i];
        i += 1;
    }
    keys.len()
}

fn fail(step: u32, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_WASM_GLUE fail step={step} {detail}\n"
    );
    qemu_exit::fail();
}
