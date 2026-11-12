//! `speaker-core` proof. Channel 2 tones, PIT latch, speaker gate, and the
//! 100 Hz timer still advancing. Safe code only.

use core::fmt::Write;

use crate::frames;
use crate::heap;
use crate::irq;
use crate::qemu_exit;
use crate::serial;
use crate::speaker::{self, Played};

const HZ_LOW: u32 = 500;
const HZ_HIGH: u32 = 2000;
const DIV_LOW: u16 = 2386;
const DIV_HIGH: u16 = 596;
/// Mode 3 counts down by two per PIT clock. On QEMU 8.2 the latch command
/// runs three clocks after the high byte is written, so the countdown is
/// the reload value minus 6. The gap is the fixed port-I/O sequence in
/// `program_and_latch`, and the proof requires this exact readback.
const LATCH_LAG: u16 = 6;

pub fn run() -> ! {
    let heap_before = heap::used();
    let free_before = frames::stats().1;
    let _ = write!(serial::Writer, "R3ATH3_SPEAKER_CORE start\n");

    let off = speaker::tone(0);
    if off.hz != 0 || off.divisor != 0 || off.latched != 0 || off.gate != 0 {
        fail_play(1, &off);
    }
    let _ = write!(serial::Writer, "R3ATH3_SPEAKER_CORE step=1 off hz=0 gate=0\n");

    let low = speaker::tone(HZ_LOW);
    if low.hz != HZ_LOW
        || low.divisor != DIV_LOW
        || low.latched != DIV_LOW - LATCH_LAG
        || low.gate != 3
    {
        fail_play(2, &low);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE step=2 hz=500 divisor=2386 latched=2380 gate=3\n"
    );

    let ticks = irq::wait_ticks(5);
    if ticks != 5 {
        fail_ticks(ticks);
    }
    let _ = write!(serial::Writer, "R3ATH3_SPEAKER_CORE step=3 ticks=5\n");

    let high = speaker::tone(HZ_HIGH);
    if high.hz != HZ_HIGH
        || high.divisor != DIV_HIGH
        || high.latched != DIV_HIGH - LATCH_LAG
        || high.gate != 3
    {
        fail_play(4, &high);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE step=4 hz=2000 divisor=596 latched=590 gate=3\n"
    );

    let done = speaker::tone(0);
    if done.hz != 0 || done.divisor != 0 || done.gate != 0 {
        fail_play(5, &done);
    }
    let _ = write!(serial::Writer, "R3ATH3_SPEAKER_CORE step=5 off hz=0 gate=0\n");

    if heap::used() != heap_before || frames::stats().1 != free_before {
        fail(6, "changed");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE step=6 heap=same frames=same\n"
    );
    let _ = write!(serial::Writer, "R3ATH3_SPEAKER_CORE ok\n");
    qemu_exit::speaker_core_ok();
}

fn fail(step: u8, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE fail step={step} detail={detail}\n"
    );
    qemu_exit::fail();
}

fn fail_play(step: u8, played: &Played) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE fail step={step} detail=play hz={} divisor={} latched={} gate={}\n",
        played.hz, played.divisor, played.latched, played.gate
    );
    qemu_exit::fail();
}

fn fail_ticks(ticks: u64) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_SPEAKER_CORE fail step=3 detail=ticks got={ticks}\n"
    );
    qemu_exit::fail();
}
