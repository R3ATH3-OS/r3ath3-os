//! `fb-core` proof. Mode 13h, software-buffer blit, DAC, guest `r3_pixels`,
//! then text mode restored. Safe code only.

use core::fmt::Write;

use crate::apps::{self, HELLO};
use crate::fb::{self, Format};
use crate::frames;
use crate::heap;
use crate::qemu_exit;
use crate::serial;
use crate::wasm::{self, LoadError, Trap};

/// Guest buffer: byte `i` is `i % 251`. See the WAT on `PIXELS`.
const GUEST_SUM: u32 = 7_999_385;
const DIRECT_SUM: u32 = 8_160_000;

pub fn run() -> ! {
    let heap_before = heap::used();
    let free_before = frames::stats().1;
    let _ = write!(serial::Writer, "R3ATH3_FB_CORE start\n");

    fb::enter_graphics();
    let view = fb::info();
    let buf_ptr = fb::bytes().as_mut_ptr();
    let buf_len = fb::bytes().len();
    if view.width != fb::WIDTH
        || view.height != fb::HEIGHT
        || view.stride != fb::STRIDE
        || view.format != Format::Indexed8
        || view.base.is_null()
        || view.base != buf_ptr
        || buf_len != fb::LEN
        || !fb::in_graphics()
    {
        fail(1, "info");
    }
    let gc6 = fb::gc(0x06);
    let crtc9 = fb::crtc(0x09);
    if gc6 != 0x05 || crtc9 != 0x41 {
        fail_regs(1, gc6, crtc9);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=1 mode=13h width=320 height=200 stride=320 format=indexed8 gc6=05 crtc9=41\n"
    );

    {
        let buf = fb::bytes();
        for (index, byte) in buf.iter_mut().enumerate() {
            *byte = direct_byte(index);
        }
    }
    fb::blit();
    if fb::hardware_sum() != DIRECT_SUM
        || fb::hw_byte(0) != Some(direct_byte(0))
        || fb::hw_byte(319) != Some(direct_byte(319))
        || fb::hw_byte(63999) != Some(direct_byte(63999))
    {
        fail_sum(2, fb::hardware_sum(), DIRECT_SUM);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=2 direct sum=8160000 px0=1 px319=190 px63999=254\n"
    );

    let d0 = fb::dac(0);
    let d1 = fb::dac(1);
    let d255 = fb::dac(255);
    if d0 != fb::palette(0) || d1 != fb::palette(1) || d255 != fb::palette(255) {
        fail_dac(d0, d1, d255);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=3 dac0=0,0,0 dac1=0,0,21 dac255=63,63,63\n"
    );

    match load_pixels() {
        Ok(()) => {}
        Err(detail) => fail(4, detail),
    }
    if fb::hardware_sum() != GUEST_SUM
        || fb::hw_byte(0) != Some(0)
        || fb::hw_byte(251) != Some(0)
        || fb::hw_byte(63999) != Some(245)
        || fb::bytes()[1] != 1
    {
        fail_sum(4, fb::hardware_sum(), GUEST_SUM);
    }
    match load_hello() {
        Ok(false) => {}
        Ok(true) => fail(4, "hello-export"),
        Err(detail) => fail(4, detail),
    }
    if fb::bytes()[1] != 1 || !fb::in_graphics() {
        fail(4, "absent");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=4 guest sum=7999385 px0=0 px251=0 px63999=245 absent=ok\n"
    );

    fb::enter_text();
    let gc6 = fb::gc(0x06);
    let crtc9 = fb::crtc(0x09);
    let cell = fb::text_cell_roundtrip(0x0A41);
    if fb::in_graphics() || gc6 != 0x0E || crtc9 != 0x4F || cell != 0x0A41 {
        fail_text(gc6, crtc9, cell);
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=5 text gc6=0e crtc9=4f cell=0a41\n"
    );

    if heap::used() != heap_before || frames::stats().1 != free_before {
        fail(6, "changed");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE step=6 heap=same frames=same\n"
    );
    let _ = write!(serial::Writer, "R3ATH3_FB_CORE ok\n");
    qemu_exit::fb_core_ok();
}

fn load_pixels() -> Result<(), &'static str> {
    let outcome = wasm::with_instance(PIXELS, |loaded| match loaded {
        Ok(mut inst) => apps::blit_exported(&mut inst).map_err(trap_word),
        Err(error) => Err(error.word()),
    });
    match outcome {
        Some(Ok(true)) => Ok(()),
        Some(Ok(false)) => Err("no-export"),
        Some(Err(detail)) => Err(detail),
        None => Err(LoadError::Busy.word()),
    }
}

fn load_hello() -> Result<bool, &'static str> {
    let outcome = wasm::with_instance(HELLO, |loaded| match loaded {
        Ok(mut inst) => apps::blit_exported(&mut inst).map_err(trap_word),
        Err(error) => Err(error.word()),
    });
    match outcome {
        Some(result) => result,
        None => Err(LoadError::Busy.word()),
    }
}

fn trap_word(trap: Trap) -> &'static str {
    trap.word()
}

fn direct_byte(index: usize) -> u8 {
    (index as u8).wrapping_mul(3).wrapping_add(1)
}

fn fail(step: u8, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE fail step={step} detail={detail}\n"
    );
    qemu_exit::fail();
}

fn fail_regs(step: u8, gc6: u8, crtc9: u8) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE fail step={step} detail=regs gc6={gc6:02x} crtc9={crtc9:02x}\n"
    );
    qemu_exit::fail();
}

fn fail_sum(step: u8, got: u32, want: u32) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE fail step={step} detail=sum got={got} want={want}\n"
    );
    qemu_exit::fail();
}

fn fail_dac(d0: (u8, u8, u8), d1: (u8, u8, u8), d255: (u8, u8, u8)) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE fail step=3 detail=dac {},{},{} {},{},{} {},{},{}\n",
        d0.0, d0.1, d0.2, d1.0, d1.1, d1.2, d255.0, d255.1, d255.2
    );
    qemu_exit::fail();
}

fn fail_text(gc6: u8, crtc9: u8, cell: u16) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_FB_CORE fail step=5 detail=text gc6={gc6:02x} crtc9={crtc9:02x} cell={cell:04x}\n"
    );
    qemu_exit::fail();
}

/// ```wat
/// (module
///   (memory (export "memory") 1 1)
///   (func (export "r3_pixels") (result i32)
///     (local $i i32)
///     (loop
///       (i32.store8 (local.get $i)
///         (i32.rem_u (local.get $i) (i32.const 251)))
///       (local.set $i (i32.add (local.get $i) (i32.const 1)))
///       (br_if 0 (i32.lt_u (local.get $i) (i32.const 64000))))
///     (i32.const 0)))
/// ```
static PIXELS: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7F, 0x03,
    0x02, 0x01, 0x00, 0x05, 0x04, 0x01, 0x01, 0x01, 0x01, 0x07, 0x16, 0x02, 0x06, 0x6D, 0x65, 0x6D,
    0x6F, 0x72, 0x79, 0x02, 0x00, 0x09, 0x72, 0x33, 0x5F, 0x70, 0x69, 0x78, 0x65, 0x6C, 0x73, 0x00,
    0x00, 0x0A, 0x26, 0x01, 0x24, 0x01, 0x01, 0x7F, 0x03, 0x40, 0x20, 0x00, 0x20, 0x00, 0x41, 0xFB,
    0x01, 0x70, 0x3A, 0x00, 0x00, 0x20, 0x00, 0x41, 0x01, 0x6A, 0x21, 0x00, 0x20, 0x00, 0x41, 0x80,
    0xF4, 0x03, 0x49, 0x0D, 0x00, 0x0B, 0x41, 0x00, 0x0B,
];
