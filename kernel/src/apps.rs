//! Baked apps and the app host. See `docs/WASM.md`.
//!
//! Safe code only. No heap allocation. The two modules below are hand-encoded
//! integer-subset binaries; the comments are the WAT they were built from.

#![allow(dead_code)]

use crate::wasm::{Instance, ValType};
#[cfg(any(feature = "shell", feature = "fb-core"))]
use crate::wasm::LoadError;
#[cfg(any(feature = "shell", feature = "wasm-glue", feature = "fb-core"))]
use crate::wasm::Trap;

#[cfg(feature = "shell")]
use crate::irq::{self, Key};
#[cfg(feature = "shell")]
use crate::sched;
#[cfg(any(feature = "shell", feature = "fb-core"))]
use crate::vga;

pub struct App {
    pub name: &'static str,
    pub desc: &'static str,
    pub bytes: &'static [u8],
}

/// ```wat
/// (module
///   (memory (export "memory") 1 1)
///   (func (export "r3_abi") (result i32) i32.const 1)
///   (func (export "r3_init") (param i32))
///   (func (export "r3_key") (param i32))
///   (func (export "r3_step") (result i32) i32.const 0)
///   (func (export "r3_text") (result i32) i32.const 0)
///   (func (export "r3_summary") (result i32) i32.const 2000)
///   (data (i32.const 990) "hello from wasm")
///   (data (i32.const 1150) "press q to quit"))
/// ```
pub static HELLO: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60,
    0x00, 0x01, 0x7F, 0x60, 0x01, 0x7F, 0x00, 0x03, 0x07, 0x06, 0x00, 0x01,
    0x01, 0x00, 0x00, 0x00, 0x05, 0x04, 0x01, 0x01, 0x01, 0x01, 0x07, 0x47,
    0x07, 0x06, 0x6D, 0x65, 0x6D, 0x6F, 0x72, 0x79, 0x02, 0x00, 0x06, 0x72,
    0x33, 0x5F, 0x61, 0x62, 0x69, 0x00, 0x00, 0x07, 0x72, 0x33, 0x5F, 0x69,
    0x6E, 0x69, 0x74, 0x00, 0x01, 0x06, 0x72, 0x33, 0x5F, 0x6B, 0x65, 0x79,
    0x00, 0x02, 0x07, 0x72, 0x33, 0x5F, 0x73, 0x74, 0x65, 0x70, 0x00, 0x03,
    0x07, 0x72, 0x33, 0x5F, 0x74, 0x65, 0x78, 0x74, 0x00, 0x04, 0x0A, 0x72,
    0x33, 0x5F, 0x73, 0x75, 0x6D, 0x6D, 0x61, 0x72, 0x79, 0x00, 0x05, 0x0A,
    0x1C, 0x06, 0x04, 0x00, 0x41, 0x01, 0x0B, 0x02, 0x00, 0x0B, 0x02, 0x00,
    0x0B, 0x04, 0x00, 0x41, 0x00, 0x0B, 0x04, 0x00, 0x41, 0x00, 0x0B, 0x05,
    0x00, 0x41, 0xD0, 0x0F, 0x0B, 0x0B, 0xB7, 0x01, 0x01, 0x00, 0x41, 0xDE,
    0x07, 0x0B, 0xAF, 0x01, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x66, 0x72,
    0x6F, 0x6D, 0x20, 0x77, 0x61, 0x73, 0x6D, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x72, 0x65, 0x73,
    0x73, 0x20, 0x71, 0x20, 0x74, 0x6F, 0x20, 0x71, 0x75, 0x69, 0x74,
];

/// ```wat
/// (module
///   (memory (export "memory") 1 1)
///   (func (export "r3_abi") (result i32) i32.const 1)
///   (func (export "r3_init") (param i32))
///   (func (export "r3_key") (param i32))
///   (func (export "r3_step") (result i32)
///     (loop (br 0))
///     i32.const 0)
///   (func (export "r3_text") (result i32) i32.const 0)
///   (func (export "r3_summary") (result i32) i32.const 2000))
/// ```
pub static SPIN: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60,
    0x00, 0x01, 0x7F, 0x60, 0x01, 0x7F, 0x00, 0x03, 0x07, 0x06, 0x00, 0x01,
    0x01, 0x00, 0x00, 0x00, 0x05, 0x04, 0x01, 0x01, 0x01, 0x01, 0x07, 0x47,
    0x07, 0x06, 0x6D, 0x65, 0x6D, 0x6F, 0x72, 0x79, 0x02, 0x00, 0x06, 0x72,
    0x33, 0x5F, 0x61, 0x62, 0x69, 0x00, 0x00, 0x07, 0x72, 0x33, 0x5F, 0x69,
    0x6E, 0x69, 0x74, 0x00, 0x01, 0x06, 0x72, 0x33, 0x5F, 0x6B, 0x65, 0x79,
    0x00, 0x02, 0x07, 0x72, 0x33, 0x5F, 0x73, 0x74, 0x65, 0x70, 0x00, 0x03,
    0x07, 0x72, 0x33, 0x5F, 0x74, 0x65, 0x78, 0x74, 0x00, 0x04, 0x0A, 0x72,
    0x33, 0x5F, 0x73, 0x75, 0x6D, 0x6D, 0x61, 0x72, 0x79, 0x00, 0x05, 0x0A,
    0x21, 0x06, 0x04, 0x00, 0x41, 0x01, 0x0B, 0x02, 0x00, 0x0B, 0x02, 0x00,
    0x0B, 0x09, 0x00, 0x03, 0x40, 0x0C, 0x00, 0x0B, 0x41, 0x00, 0x0B, 0x04,
    0x00, 0x41, 0x00, 0x0B, 0x05, 0x00, 0x41, 0xD0, 0x0F, 0x0B,
];

#[cfg(any(feature = "glue-baked", feature = "wasm-glue"))]
pub static GLUE_DUTY: &[u8] = include_bytes!("../../build/apps/glue_duty.wasm");

#[cfg(any(feature = "glue-baked", feature = "wasm-glue"))]
pub static APPS: &[App] = &[
    App {
        name: "glue-duty",
        desc: "Glue Duty, the belt-scrub game, in text mode",
        bytes: GLUE_DUTY,
    },
    App {
        name: "hello",
        desc: "a static screen; the smallest app",
        bytes: HELLO,
    },
    App {
        name: "spin",
        desc: "loops forever; the fuel limit stops it",
        bytes: SPIN,
    },
];

#[cfg(not(any(feature = "glue-baked", feature = "wasm-glue")))]
pub static APPS: &[App] = &[
    App {
        name: "hello",
        desc: "a static screen; the smallest app",
        bytes: HELLO,
    },
    App {
        name: "spin",
        desc: "loops forever; the fuel limit stops it",
        bytes: SPIN,
    },
];

pub fn find(name: &[u8]) -> Option<&'static App> {
    APPS.iter().find(|app| app.name.as_bytes() == name)
}

pub fn check_abi(inst: &mut Instance<'_, '_>) -> bool {
    if !inst.export_is_memory("memory") {
        return false;
    }
    if !inst.func_matches("r3_abi", &[], Some(ValType::I32))
        || !inst.func_matches("r3_init", &[ValType::I32], None)
        || !inst.func_matches("r3_key", &[ValType::I32], None)
        || !inst.func_matches("r3_step", &[], Some(ValType::I32))
        || !inst.func_matches("r3_text", &[], Some(ValType::I32))
        || !inst.func_matches("r3_summary", &[], Some(ValType::I32))
    {
        return false;
    }
    matches!(inst.call("r3_abi", &[]), Ok(Some(1)))
}

#[cfg(feature = "shell")]
pub fn run_app(app: &'static App) -> bool {
    both(b"app ");
    both(app.name.as_bytes());
    both(b": started; q quits\n");

    let outcome = crate::wasm::with_instance(app.bytes, |loaded| match loaded {
        Err(error) => End::NotLoaded(error),
        Ok(mut inst) => host_loop(app.name, &mut inst),
    });
    let outcome = outcome.unwrap_or(End::NotLoaded(LoadError::Busy));
    let ran = matches!(outcome, End::Quit(_, _) | End::Trap(_));
    finish(app.name, outcome);
    ran
}

#[cfg(any(feature = "shell", feature = "fb-core"))]
enum End {
    NotLoaded(LoadError),
    Abi,
    Quit([u8; 60], usize),
    Trap(Trap),
}

#[cfg(feature = "shell")]
fn host_loop(name: &str, inst: &mut Instance<'_, '_>) -> End {
    if !check_abi(inst) {
        return End::Abi;
    }
    let seed = (irq::ticks() as i32) | 1;
    if let Err(trap) = inst.call("r3_init", &[i64::from(seed)]) {
        return End::Trap(trap);
    }
    if let Err(trap) = present(inst) {
        return End::Trap(trap);
    }
    // One step before the ready line, so a fuel trap (spin) is reported
    // without a state line and the hello screen is already painted.
    let state = match inst.call("r3_step", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return End::Trap(Trap::Stack),
        Err(trap) => return End::Trap(trap),
    };
    if let Err(trap) = present(inst) {
        return End::Trap(trap);
    }
    let _ = core::fmt::Write::write_fmt(
        &mut crate::serial::Writer,
        format_args!("R3ATH3_APP_READY name={name}\n"),
    );
    let mut previous = state;
    loop {
        let deadline = irq::ticks().wrapping_add(10);
        loop {
            if let Some(end) = take_key(inst) {
                return end;
            }
            if irq::ticks().wrapping_sub(deadline) < (1u64 << 63) {
                break;
            }
            sched::yield_now();
        }
        let state = match inst.call("r3_step", &[]) {
            Ok(Some(value)) => value,
            Ok(None) => return End::Trap(Trap::Stack),
            Err(trap) => return End::Trap(trap),
        };
        if state != previous {
            let _ = core::fmt::Write::write_fmt(
                &mut crate::serial::Writer,
                format_args!("R3ATH3_APP_STATE name={name} state={state}\n"),
            );
            previous = state;
        }
        if let Err(trap) = present(inst) {
            return End::Trap(trap);
        }
    }
}

#[cfg(feature = "shell")]
fn take_key(inst: &mut Instance<'_, '_>) -> Option<End> {
    let key = irq::pop_key()?;
    let code = match key {
        Key::Char(b'q') => return Some(quit_end(inst)),
        Key::Char(byte) => i64::from(byte),
        Key::Backspace => 8,
        Key::Enter => 10,
    };
    match inst.call("r3_key", &[code]) {
        Ok(_) => None,
        Err(trap) => Some(End::Trap(trap)),
    }
}

#[cfg(feature = "shell")]
fn present(inst: &mut Instance<'_, '_>) -> Result<(), Trap> {
    let offset = match inst.call("r3_text", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return Err(Trap::Memory),
        Err(trap) => return Err(trap),
    };
    let memory = inst.memory();
    let start = match offset_of(offset, memory.len(), 2000) {
        Some(start) => start,
        None => return Err(Trap::Memory),
    };
    vga::present(&memory[start..start + 2000]);
    blit_exported(inst)?;
    Ok(())
}

#[cfg(any(feature = "shell", feature = "fb-core"))]
fn finish(name: &str, end: End) {
    if crate::fb::in_graphics() {
        crate::fb::enter_text();
    }
    vga::console_clear();
    match end {
        End::NotLoaded(error) => {
            both(b"app ");
            both(name.as_bytes());
            both(b": not loaded: ");
            both(error.word().as_bytes());
            both(b"\n");
        }
        End::Abi => {
            both(b"app ");
            both(name.as_bytes());
            both(b": stopped: abi\n");
        }
        End::Quit(text, len) => {
            both(b"app ");
            both(name.as_bytes());
            both(b": quit");
            if len > 0 {
                both(b"; ");
                for &byte in &text[..len] {
                    both(&[printable(byte)]);
                }
            }
            both(b"\n");
        }
        End::Trap(trap) => {
            both(b"app ");
            both(name.as_bytes());
            both(b": stopped: trap ");
            both(trap.word().as_bytes());
            both(b"\n");
        }
    }
}

/// `fb-core` leaves graphics through the same `finish` the shell uses.
#[cfg(feature = "fb-core")]
pub(crate) fn finish_for_proof() {
    finish("pixels", End::Quit([0; 60], 0));
}

#[cfg(feature = "shell")]
fn quit_end(inst: &mut Instance<'_, '_>) -> End {
    match read_summary(inst) {
        Ok(summary) => End::Quit(summary.0, summary.1),
        Err(trap) => End::Trap(trap),
    }
}

#[cfg(any(feature = "shell", feature = "wasm-glue"))]
pub(crate) fn read_summary(inst: &mut Instance<'_, '_>) -> Result<([u8; 60], usize), Trap> {
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
    if len == 0 {
        return Ok(([0; 60], 0));
    }
    if len > 60 {
        return Err(Trap::Memory);
    }
    let Some(body) = offset_of(offset + 1, memory.len(), len) else {
        return Err(Trap::Memory);
    };
    let mut text = [0u8; 60];
    text[..len].copy_from_slice(&memory[body..body + len]);
    Ok((text, len))
}

#[cfg(any(feature = "shell", feature = "fb-core"))]
fn both(bytes: &[u8]) {
    crate::serial::write_bytes(bytes);
    for &byte in bytes {
        crate::vga::console_put(byte);
    }
}

/// Guest summary bytes on the way to serial or VGA. `0x00` is a space.
/// Bytes outside `0x20..=0x7E` are `?`. Printable bytes pass through.
#[cfg(any(feature = "shell", feature = "wasm-glue", feature = "fb-core"))]
pub(crate) fn printable(byte: u8) -> u8 {
    if byte == 0x00 {
        b' '
    } else if (0x20..=0x7E).contains(&byte) {
        byte
    } else {
        b'?'
    }
}

/// Copy an exported `r3_pixels` buffer into the framebuffer and blit it.
/// `Ok(false)` means the module does not export `r3_pixels`. Apps without
/// that export stay on the text path.
#[cfg(any(feature = "shell", feature = "fb-core"))]
pub fn blit_exported(inst: &mut Instance<'_, '_>) -> Result<bool, Trap> {
    if !inst.func_matches("r3_pixels", &[], Some(ValType::I32)) {
        return Ok(false);
    }
    let offset = match inst.call("r3_pixels", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return Err(Trap::Memory),
        Err(trap) => return Err(trap),
    };
    let memory = inst.memory();
    let Some(start) = offset_of(offset, memory.len(), crate::fb::LEN) else {
        return Err(Trap::Memory);
    };
    if !crate::fb::in_graphics() {
        crate::fb::enter_graphics();
    }
    if !crate::fb::copy_from(&memory[start..start + crate::fb::LEN]) {
        return Err(Trap::Memory);
    }
    crate::fb::blit();
    Ok(true)
}

#[cfg(any(feature = "shell", feature = "wasm-glue", feature = "fb-core"))]
pub(crate) fn offset_of(offset: i64, len: usize, span: usize) -> Option<usize> {
    if offset < 0 {
        return None;
    }
    let start = offset as usize;
    if start.checked_add(span).map(|end| end <= len) != Some(true) {
        return None;
    }
    Some(start)
}
