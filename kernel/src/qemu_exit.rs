//! Exit a QEMU process through the `isa-debug-exit` device.
//!
//! QEMU turns an `out` of `V` to port `0xF4` into process status `(V << 1) | 1`.
//! `0x10` therefore becomes status 33 (pass) and `0x11` becomes status 35 (fail).
//! CPU faults use `0x30` (#DE → 97), `0x31` (#UD → 99), `0x32` (#GP → 101),
//! `0x33` (#PF → 103), `0x34` (#DF → 105). Any other vector uses `0x3E` (125).
//! This device exists only in the emulator. Do not boot this image on hardware.

use crate::port;

const DEBUG_EXIT_PORT: u16 = 0xF4;
#[allow(dead_code)] // panic and hang builds do not report success
pub const SUCCESS: u32 = 0x10;
pub const FAILED: u32 = 0x11;

#[allow(dead_code)] // panic and hang builds exit by another path
pub fn success() -> ! {
    exit(SUCCESS)
}

pub fn fail() -> ! {
    exit(FAILED)
}

pub fn cpu_fault(vector: u64) -> ! {
    let code = match vector {
        0 => 0x30,
        6 => 0x31,
        13 => 0x32,
        14 => 0x33,
        8 => 0x34,
        _ => 0x3E,
    };
    exit(code);
}

fn exit(code: u32) -> ! {
    unsafe {
        port::outl(DEBUG_EXIT_PORT, code);
    }
    halt_forever()
}

pub fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
