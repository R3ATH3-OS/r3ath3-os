//! PC speaker on PIT channel 2.
//!
//! A frequency word programs a square wave. Zero means off: the speaker gate
//! closes and channel 2 is not reprogrammed. Channel 0 is the 100 Hz timer
//! (port `0x40`, divisor 11932) and is not touched. Command-port writes use
//! channel-2 bytes only (`0xB6` to program, `0x80` to latch). The gate is
//! port `0x61`: only bits 0 and 1 change. Ports `0x60` and `0x64` are not used.
//!
//! On QEMU 8.2 the latched countdown is the reload value minus 6: mode 3
//! steps by two per clock, and three clocks pass between the high-byte write
//! and the latch command. `speaker-core` requires that exact readback.
//!
//! # Invariants
//! One CPU. Main context only. There is no PCM, AC97, or HD Audio device.

#![allow(dead_code)]

use crate::port;

const PIT_CH2: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const SPEAKER: u16 = 0x61;

/// Channel 2, low byte then high byte, mode 3 (square wave), binary.
const CMD_CH2_SQUARE: u8 = 0xB6;
/// Channel 2 latch-count command. Does not select channel 0.
const CMD_CH2_LATCH: u8 = 0x80;

const PIT_HZ: u32 = 1_193_182;

const _: () = assert!(CMD_CH2_SQUARE & 0xC0 == 0x80);
const _: () = assert!(CMD_CH2_LATCH & 0xC0 == 0x80);
const _: () = assert!(PIT_HZ / 500 == 2386);
const _: () = assert!(PIT_HZ / 2000 == 596);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Played {
    pub hz: u32,
    pub divisor: u16,
    /// Countdown latched from channel 2. Zero when `hz` is zero (no latch).
    pub latched: u16,
    /// Port `0x61` bits 0 and 1 after the update.
    pub gate: u8,
}

/// Reload value for `hz`. Zero stays zero. Any other word becomes a divisor
/// in `1..=65535` by integer division of the 1_193_182 Hz PIT clock.
pub fn divisor(hz: u32) -> u16 {
    if hz == 0 {
        return 0;
    }
    let raw = PIT_HZ / hz;
    if raw == 0 {
        1
    } else if raw > 65535 {
        65535
    } else {
        raw as u16
    }
}

/// Play `hz`, or silence the speaker when `hz` is zero.
pub fn tone(hz: u32) -> Played {
    if hz == 0 {
        return Played {
            hz: 0,
            divisor: 0,
            latched: 0,
            gate: set_gate(false),
        };
    }
    let _ = set_gate(false);
    let divisor = divisor(hz);
    let latched = program_and_latch(divisor);
    Played {
        hz,
        divisor,
        latched,
        gate: set_gate(true),
    }
}

/// Close the speaker gate. Channel 0 is left as the timer programmed it.
pub fn silence() {
    let _ = tone(0);
}

fn program_and_latch(divisor: u16) -> u16 {
    let enable = interrupts_on();
    // SAFETY: `cli` clears IF on this one CPU so the latch is not split by
    // the timer handler. `sti` below restores IF only when it was set.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    // SAFETY: `0x43` is written only with the channel-2 command bytes
    // `0xB6` and `0x80`. `0x42` is channel 2's data port. Port `0x40` is
    // not used, so channel 0's divisor is not rewritten. One CPU.
    let latched = unsafe {
        port::outb(PIT_CMD, CMD_CH2_SQUARE);
        port::outb(PIT_CH2, divisor as u8);
        port::outb(PIT_CH2, (divisor >> 8) as u8);
        port::outb(PIT_CMD, CMD_CH2_LATCH);
        let low = port::inb(PIT_CH2);
        let high = port::inb(PIT_CH2);
        u16::from(low) | (u16::from(high) << 8)
    };
    if enable {
        // SAFETY: IF was set on entry. `sti` puts it back. One CPU.
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    latched
}

fn set_gate(on: bool) -> u8 {
    // SAFETY: port `0x61` bits 0 and 1 are the channel-2 gate and the speaker
    // data. The value written keeps every other bit from the read. Ports
    // `0x60` and `0x64` are not used. One CPU, main context only.
    unsafe {
        let before = port::inb(SPEAKER);
        let bits = if on { 0x03 } else { 0x00 };
        port::outb(SPEAKER, (before & 0xFC) | bits);
        port::inb(SPEAKER) & 0x03
    }
}

fn interrupts_on() -> bool {
    let flags: u64;
    // SAFETY: `pushfq`/`pop` copies RFLAGS and restores it. The pair does
    // not write Rust memory. One CPU.
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            flags = out(reg) flags,
            options(nomem, preserves_flags),
        );
    }
    flags & 0x200 != 0
}
