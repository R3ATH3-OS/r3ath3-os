//! PC speaker on PIT channel 2.
//!
//! A frequency word programs a square wave. Zero means off: the speaker gate
//! closes and channel 2 is not reprogrammed. Channel 0 is the 100 Hz timer
//! (port `0x40`, divisor 11932) and is not touched. Command-port writes use
//! channel-2 bytes only (`0xB6` to program, `0x80` to latch). The gate is
//! port `0x61`: only bits 0 and 1 change. Ports `0x60` and `0x64` are not used.
//!
//! Mode 3 counts down by two per PIT clock, so a latched count is even when
//! the divisor is even. The latch is read in the same instruction window as
//! the reload write. `speaker-core` accepts an even count in
//! `divisor - 20 .. divisor`. The raw count is printed only when that check
//! fails. Host time can move the count inside that window; the proof does
//! not require one exact lag.
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

/// Channel-2 count and speaker-gate bits, without programming a divisor
/// and without changing the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub latched: u16,
    pub gate: u8,
}

/// Latch channel 2 and read port `0x61` bits 0 and 1. Does not write a
/// divisor and does not change the gate. Channel 0 and the PS/2 ports
/// stay untouched.
pub fn sample() -> Sample {
    Sample {
        latched: channel2(None),
        gate: gate(None),
    }
}

fn program_and_latch(divisor: u16) -> u16 {
    channel2(Some(divisor))
}

fn channel2(program: Option<u16>) -> u16 {
    crate::irq::without_interrupts(|| {
        // SAFETY: `0x43` is written only with the channel-2 command bytes
        // `0xB6` and `0x80`. `0x42` is channel 2's data port. Port `0x40` is
        // not used, so channel 0's divisor is not rewritten. The surrounding
        // `without_interrupts` keeps the timer handler from splitting the
        // latch. A read passes `program == None` and sends only the latch
        // command. Every access in this block is port I/O. One CPU.
        unsafe {
            if let Some(divisor) = program {
                port::outb(PIT_CMD, CMD_CH2_SQUARE);
                port::outb(PIT_CH2, divisor as u8);
                port::outb(PIT_CH2, (divisor >> 8) as u8);
            }
            port::outb(PIT_CMD, CMD_CH2_LATCH);
            let low = port::inb(PIT_CH2);
            let high = port::inb(PIT_CH2);
            u16::from(low) | (u16::from(high) << 8)
        }
    })
}

fn set_gate(on: bool) -> u8 {
    gate(Some(on))
}

fn gate(on: Option<bool>) -> u8 {
    // SAFETY: port `0x61` bits 0 and 1 are the channel-2 gate and the speaker
    // data. The value written keeps every other bit from the read. A read
    // passes `on == None` and does not write the port. Ports `0x60` and
    // `0x64` are not used. One CPU, main context only.
    unsafe {
        let before = port::inb(SPEAKER);
        if let Some(on) = on {
            let bits = if on { 0x03 } else { 0x00 };
            port::outb(SPEAKER, (before & 0xFC) | bits);
        }
        port::inb(SPEAKER) & 0x03
    }
}
