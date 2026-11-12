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
            let (summary, summary_len) = match summary_text(&mut inst) {
                Ok(text) => text,
                Err(trap) => fail(n, trap.word()),
            };
            if state != prev || n % 100 == 0 {
                let _ = write!(
                    serial::Writer,
                    "R3ATH3_WASM_GLUE step={n} state={state} frame={frame:016x} "
                );
                serial::write_bytes(&summary[..summary_len]);
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
    let Some(start) = offset_of(offset, memory.len(), 2000) else {
        return Err(Trap::Memory);
    };
    Ok(fnv64(&memory[start..start + 2000]))
}

fn summary_text(inst: &mut wasm::Instance<'_, '_>) -> Result<([u8; 60], usize), Trap> {
    let offset = match inst.call("r3_summary", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return Err(Trap::Memory),
        Err(trap) => return Err(trap),
    };
    let memory = inst.memory();
    let Some(start) = offset_of(offset, memory.len(), 1) else {
        return Err(Trap::Memory);
    };
    let len = memory[start] as usize;
    if len > 60 {
        return Err(Trap::Memory);
    }
    let mut text = [0u8; 60];
    if len == 0 {
        return Ok((text, 0));
    }
    let Some(body) = offset_of(offset + 1, memory.len(), len) else {
        return Err(Trap::Memory);
    };
    text[..len].copy_from_slice(&memory[body..body + len]);
    Ok((text, len))
}

fn offset_of(offset: i64, len: usize, span: usize) -> Option<usize> {
    if offset < 0 {
        return None;
    }
    let start = offset as usize;
    if start.checked_add(span).map(|end| end <= len) != Some(true) {
        None
    } else {
        Some(start)
    }
}

fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Keys applied before step `n`. Keep this in step with `replay.rs`.
fn proof_keys(n: u32, out: &mut [u8; 4]) -> usize {
    let mut len = 0;
    if n == 1 || n == 1001 {
        out[len] = b' ';
        len += 1;
    }
    if (2..=740).contains(&n) || (1002..=1400).contains(&n) {
        if n % 60 == 0 {
            out[len] = if (n / 60) % 2 == 0 { b'w' } else { b's' };
            len += 1;
        }
        if n % 150 == 75 {
            out[len] = b' ';
            len += 1;
        }
        if n == 300 || n == 310 {
            out[len] = b'p';
            len += 1;
        }
        out[len] = if n % 2 == 0 { b'a' } else { b'd' };
        len += 1;
    }
    len
}

fn fail(step: u32, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_WASM_GLUE fail step={step} {detail}\n"
    );
    qemu_exit::fail();
}
