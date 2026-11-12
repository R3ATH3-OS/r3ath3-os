//! Exit a QEMU process through the `isa-debug-exit` device.
//!
//! QEMU turns an `out` of `V` to port `0xF4` into process status `(V << 1) | 1`.
//! `0x10` therefore becomes status 33 (pass) and `0x11` becomes status 35 (fail).
//! CPU faults use `0x30` (#DE → 97), `0x31` (#UD → 99), `0x32` (#GP → 101),
//! `0x33` (#PF → 103), `0x34` (#DF → 105). Any other vector uses `0x2F` (95),
//! which sits outside GNU timeout's own statuses 124–127.
//! A COM1 task script writes `0x50` (ok → 161) or `0x51` (error → 163).
//! Timer proof writes `0x58` (177). Keyboard echo writes `0x59` (179).
//! A keyboard task script writes `0x5A` (181) on success.
//! Allocator proofs: heap ok `0x60` (193), heap exhaustion `0x61` (195), frame
//! allocator ok `0x62` (197), frame exhaustion `0x63` (199). A memory
//! initialisation error writes `0x64` (201). The heap-overflow panic proof
//! reuses the panic status 35 on purpose.
//! The shell writes `0x68` (exit ok → 209), `0x69` (command error → 211),
//! `0x6A` (line overflow → 213), `0x6B` (script error → 215), or `0x6C`
//! (halt → 217). Scheduler proofs write `0x70` (coop → 225), `0x71`
//! (preempt → 227), `0x72` (sleep → 229), `0x73` (full → 231), `0x74`
//! (exit → 233), and `0x75` (reap → 235). The gate proofs write `0x78`
//! (core → 241) and `0x79` (expire → 243). The flight-recorder proof writes
//! `0x7A` (core → 245). The store proof writes `0x7B` (core → 247). The NVMe
//! proof writes `0x7C` (core → 249). With an NVMe disk the shell writes the
//! live record through to that disk, and an earlier boot is read with
//! `boots K`. Without a disk the shell uses the RAM disk, and a reboot erases
//! both. Unused odd statuses: `0x65`–`0x67` (203–207), `0x6D`–`0x6F`
//! (219–223), `0x76`–`0x77` (237–239), `0x7D` (251), `0x7E` (253), and
//! `0x7F` (255).
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

#[cfg(feature = "script")]
pub fn script_ok() -> ! {
    exit(0x50)
}

#[cfg(any(feature = "script", feature = "kbd-script"))]
pub fn script_error() -> ! {
    exit(0x51)
}

#[cfg(feature = "kbd-script")]
pub fn kbd_script_ok() -> ! {
    exit(0x5A)
}

#[cfg(feature = "ticks")]
pub fn ticks_ok() -> ! {
    exit(0x58)
}

#[cfg(feature = "keys")]
pub fn keys_ok() -> ! {
    exit(0x59)
}

#[cfg(feature = "heap")]
pub fn heap_ok() -> ! {
    exit(0x60)
}

#[cfg(feature = "heap-oom")]
pub fn heap_oom_ok() -> ! {
    exit(0x61)
}

#[cfg(feature = "frames")]
pub fn frames_ok() -> ! {
    exit(0x62)
}

#[cfg(feature = "frames-oom")]
pub fn frames_oom_ok() -> ! {
    exit(0x63)
}

pub fn mem_error() -> ! {
    exit(0x64)
}

#[cfg(feature = "shell")]
pub fn shell_exit_ok() -> ! {
    exit(0x68)
}

#[cfg(feature = "shell")]
pub fn shell_exit_cmd() -> ! {
    exit(0x69)
}

#[cfg(feature = "shell")]
pub fn shell_exit_overflow() -> ! {
    exit(0x6A)
}

#[cfg(feature = "shell")]
pub fn shell_exit_script() -> ! {
    exit(0x6B)
}

#[cfg(feature = "shell")]
pub fn shell_halt() -> ! {
    exit(0x6C)
}

#[cfg(feature = "sched-coop")]
pub fn sched_coop_ok() -> ! {
    exit(0x70)
}

#[cfg(feature = "sched-preempt")]
pub fn sched_preempt_ok() -> ! {
    exit(0x71)
}

#[cfg(feature = "sched-sleep")]
pub fn sched_sleep_ok() -> ! {
    exit(0x72)
}

#[cfg(feature = "sched-full")]
pub fn sched_full_ok() -> ! {
    exit(0x73)
}

#[cfg(feature = "sched-exit")]
pub fn sched_exit_ok() -> ! {
    exit(0x74)
}

#[cfg(feature = "sched-reap")]
pub fn sched_reap_ok() -> ! {
    exit(0x75)
}

#[cfg(feature = "gate-core")]
pub fn gate_core_ok() -> ! {
    exit(0x78)
}

#[cfg(feature = "gate-expire")]
pub fn gate_expire_ok() -> ! {
    exit(0x79)
}

#[cfg(feature = "recorder-core")]
pub fn recorder_core_ok() -> ! {
    exit(0x7A)
}

#[cfg(feature = "store-core")]
pub fn store_core_ok() -> ! {
    exit(0x7B)
}

#[cfg(feature = "nvme-core")]
pub fn nvme_core_ok() -> ! {
    exit(0x7C)
}

pub fn cpu_fault(vector: u64) -> ! {
    let code = match vector {
        0 => 0x30,
        6 => 0x31,
        13 => 0x32,
        14 => 0x33,
        8 => 0x34,
        _ => 0x2F,
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
