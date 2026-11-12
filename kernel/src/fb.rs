//! VGA mode 13h framebuffer.
//!
//! 320×200, 256 colors. A 64,000-byte software buffer lives in `.bss`, not
//! on the heap. `blit` copies it to `0xA0000`, which is already inside the
//! 1 GiB identity map. The frame allocator never hands out frames below 1 MiB.
//!
//! `info` is the backend-neutral view (pointer, width, height, stride,
//! format). Milestone 13 can point that view at another device. This
//! milestone does not program a Bochs/VBE linear framebuffer: that window
//! usually sits above the 1 GiB map. Higher resolution is later.
//!
//! Graphics mode is only while a pixel path runs. `enter_text` restores VGA
//! text mode 3 (80×25). The shell stays a text shell.
//!
//! # Invariants
//! One CPU. Main context only. IRQ handlers do not enter this module.
//! Port I/O here is the VGA set: misc `0x3C2`, sequencer `0x3C4`/`0x3C5`,
//! CRTC `0x3D4`/`0x3D5`, graphics `0x3CE`/`0x3CF`, attribute `0x3C0`, input
//! status `0x3DA`, and the DAC `0x3C7`/`0x3C8`/`0x3C9`. No PIT port and no
//! PS/2 port.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::port;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;
pub const STRIDE: usize = 320;
pub const LEN: usize = WIDTH * HEIGHT;

const _: () = assert!(LEN == 64_000);
const _: () = assert!(STRIDE == WIDTH);

const HW_BASE: usize = 0xA0000;
const TEXT_BASE: *mut u16 = 0xB8000 as *mut u16;

const MISC_WRITE: u16 = 0x3C2;
const SEQ_INDEX: u16 = 0x3C4;
const SEQ_DATA: u16 = 0x3C5;
const CRTC_INDEX: u16 = 0x3D4;
const CRTC_DATA: u16 = 0x3D5;
const GC_INDEX: u16 = 0x3CE;
const GC_DATA: u16 = 0x3CF;
const AC_WRITE: u16 = 0x3C0;
const INSTAT: u16 = 0x3DA;
const DAC_READ_INDEX: u16 = 0x3C7;
const DAC_WRITE_INDEX: u16 = 0x3C8;
const DAC_DATA: u16 = 0x3C9;

/// 8-bit palette index, one byte per pixel, row-major.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Indexed8,
}

#[derive(Clone, Copy)]
pub struct FbInfo {
    pub base: *mut u8,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: Format,
}

struct FrameBuf(UnsafeCell<[u8; LEN]>);

// SAFETY: one CPU. Only the main context borrows the buffer. IRQ handlers
// do not enter this module, and a preemption cannot run a second borrower
// because no other task calls in. The shell task is the only shell caller.
unsafe impl Sync for FrameBuf {}

static BUFFER: FrameBuf = FrameBuf(UnsafeCell::new([0; LEN]));
static GRAPHICS: AtomicBool = AtomicBool::new(false);

/// Mode 13h. Misc, five sequencer, 25 CRTC, nine graphics, 21 attribute.
const MODE_13H: [u8; 61] = [
    0x63, 0x03, 0x01, 0x0F, 0x00, 0x0E, 0x5F, 0x4F, 0x50, 0x82, 0x54, 0x80, 0xBF, 0x1F, 0x00, 0x41,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x40, 0x96, 0xB9, 0xA3, 0xFF, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x05, 0x0F, 0xFF, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x41, 0x00, 0x0F, 0x00, 0x00,
];

/// VGA text mode 3, 80×25. Same register groups as `MODE_13H`.
const MODE_TEXT: [u8; 61] = [
    0x67, 0x03, 0x00, 0x03, 0x00, 0x02, 0x5F, 0x4F, 0x50, 0x82, 0x55, 0x81, 0xBF, 0x1F, 0x00, 0x4F,
    0x0D, 0x0E, 0x00, 0x00, 0x00, 0x50, 0x9C, 0x8E, 0x8F, 0x28, 0x1F, 0x96, 0xB9, 0xA3, 0xFF, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x10, 0x0E, 0x0F, 0xFF, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07,
    0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x0C, 0x00, 0x0F, 0x08, 0x00,
];

pub fn info() -> FbInfo {
    FbInfo {
        base: BUFFER.0.get() as *mut u8,
        width: WIDTH,
        height: HEIGHT,
        stride: STRIDE,
        format: Format::Indexed8,
    }
}

pub fn bytes() -> &'static mut [u8] {
    // SAFETY: one CPU, main context only. The caller drops this slice before
    // the next borrow. IRQ handlers do not enter this module.
    unsafe { &mut *BUFFER.0.get() }
}

pub fn in_graphics() -> bool {
    GRAPHICS.load(Ordering::Relaxed)
}

/// Program mode 13h and the stable DAC palette. Does not clear the buffer.
pub fn enter_graphics() {
    write_regs(&MODE_13H);
    write_palette();
    GRAPHICS.store(true, Ordering::Relaxed);
}

/// Program VGA text mode 3 (80×25). Does not clear the text buffer.
pub fn enter_text() {
    write_regs(&MODE_TEXT);
    GRAPHICS.store(false, Ordering::Relaxed);
}

/// Copy `src` into the software buffer. `src` must be exactly `LEN` bytes.
pub fn copy_from(src: &[u8]) -> bool {
    if src.len() != LEN {
        return false;
    }
    bytes().copy_from_slice(src);
    true
}

/// Copy the software buffer to `0xA0000`.
pub fn blit() {
    let src = bytes();
    // SAFETY: `0xA0000` is the VGA window inside the identity map. `src` is
    // the 64,000-byte software buffer, so every index stays inside that
    // 64 KiB window. One CPU, main context only.
    unsafe {
        let hw = HW_BASE as *mut u8;
        for (index, byte) in src.iter().enumerate() {
            hw.add(index).write_volatile(*byte);
        }
    }
}

pub fn hw_byte(index: usize) -> Option<u8> {
    if index >= LEN {
        return None;
    }
    // SAFETY: `index < 64000`, so the address stays inside the VGA window
    // at `0xA0000`. One CPU, main context only.
    unsafe { Some((HW_BASE as *const u8).add(index).read_volatile()) }
}

pub fn hardware_sum() -> u32 {
    let mut sum = 0u32;
    // SAFETY: the loop index stays below 64000, inside the VGA window at
    // `0xA0000`. One CPU, main context only.
    unsafe {
        let hw = HW_BASE as *const u8;
        for index in 0..LEN {
            sum = sum.wrapping_add(u32::from(hw.add(index).read_volatile()));
        }
    }
    sum
}

/// DAC entry written by `enter_graphics`. Six-bit channels.
/// `r` is bits 7–5 of the index, `g` is bits 4–2, `b` is bits 1–0.
pub fn palette(index: u8) -> (u8, u8, u8) {
    let r = (index >> 5) * 9;
    let g = ((index >> 2) & 7) * 9;
    let b = (index & 3) * 21;
    (r, g, b)
}

pub fn dac(index: u8) -> (u8, u8, u8) {
    // SAFETY: `0x3C7` selects a DAC entry and `0x3C9` reads its three 6-bit
    // channels. One CPU, main context only. Not a PIT or PS/2 port.
    unsafe {
        port::outb(DAC_READ_INDEX, index);
        let r = port::inb(DAC_DATA) & 0x3F;
        let g = port::inb(DAC_DATA) & 0x3F;
        let b = port::inb(DAC_DATA) & 0x3F;
        (r, g, b)
    }
}

pub fn gc(index: u8) -> u8 {
    // SAFETY: `0x3CE`/`0x3CF` are the VGA graphics-controller index and data
    // ports. `index` is masked to the register file. One CPU, main context only.
    unsafe {
        port::outb(GC_INDEX, index & 0x0F);
        port::inb(GC_DATA)
    }
}

pub fn crtc(index: u8) -> u8 {
    // SAFETY: `0x3D4`/`0x3D5` are the VGA CRTC index and data ports. One CPU,
    // main context only.
    unsafe {
        port::outb(CRTC_INDEX, index);
        port::inb(CRTC_DATA)
    }
}

/// Write one text cell at the origin and read it back.
pub fn text_cell_roundtrip(value: u16) -> u16 {
    // SAFETY: `0xB8000` is the VGA text buffer, inside the identity map.
    // One cell, so the store stays in the first word of that buffer. One CPU,
    // main context only. The proof calls this after text mode is restored.
    unsafe {
        TEXT_BASE.write_volatile(value);
        TEXT_BASE.read_volatile()
    }
}

fn write_palette() {
    // SAFETY: `0x3C8` selects the DAC write index and `0x3C9` takes three
    // 6-bit channels per entry. The index auto-increments. One CPU, main
    // context only.
    unsafe {
        port::outb(DAC_WRITE_INDEX, 0);
        for index in 0..=255 {
            let (r, g, b) = palette(index);
            port::outb(DAC_DATA, r);
            port::outb(DAC_DATA, g);
            port::outb(DAC_DATA, b);
        }
    }
}

fn write_regs(regs: &[u8; 61]) {
    // SAFETY: VGA mode-set ports only (misc, sequencer, CRTC, graphics,
    // attribute, input status). One CPU, main context only. This does not
    // touch the PIT or the PS/2 controller.
    unsafe {
        port::outb(MISC_WRITE, regs[0]);
        for index in 0..5 {
            port::outb(SEQ_INDEX, index as u8);
            port::outb(SEQ_DATA, regs[1 + index]);
        }
        port::outb(CRTC_INDEX, 0x03);
        port::outb(CRTC_DATA, port::inb(CRTC_DATA) | 0x80);
        port::outb(CRTC_INDEX, 0x11);
        port::outb(CRTC_DATA, port::inb(CRTC_DATA) & !0x80);
        for index in 0..25 {
            let mut value = regs[6 + index];
            if index == 0x03 {
                value |= 0x80;
            }
            if index == 0x11 {
                value &= !0x80;
            }
            port::outb(CRTC_INDEX, index as u8);
            port::outb(CRTC_DATA, value);
        }
        for index in 0..9 {
            port::outb(GC_INDEX, index as u8);
            port::outb(GC_DATA, regs[31 + index]);
        }
        for index in 0..21 {
            let _ = port::inb(INSTAT);
            port::outb(AC_WRITE, index as u8);
            port::outb(AC_WRITE, regs[40 + index]);
        }
        let _ = port::inb(INSTAT);
        port::outb(AC_WRITE, 0x20);
    }
}
