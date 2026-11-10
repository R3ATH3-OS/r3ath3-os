//! Direct write to the VGA text buffer at physical `0xB8000`.
//!
//! # Invariants
//! Boot code identity-maps the first 1 GiB before `kernel_main`, so this
//! physical address is the same virtual address. Only the boot processor
//! writes it. IRQ handlers do not. `write_marker` fills 80×25 cells and
//! `write_line` fills the first row.
//! QEMU's default VGA text mode displays this buffer. No hardware scanout is
//! programmed.

// Fault builds do not call the marker writer.
#![allow(dead_code)]

const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const CELLS: usize = WIDTH * HEIGHT;
const BUFFER: *mut u16 = 0xB8000 as *mut u16;
const GREEN_ON_BLACK: u16 = 0x0A00;

pub fn write_marker(text: &str) {
    unsafe {
        let cells = core::slice::from_raw_parts_mut(BUFFER, CELLS);
        for cell in cells.iter_mut() {
            *cell = GREEN_ON_BLACK | u16::from(b' ');
        }
        let mut column = 0;
        for byte in text.bytes() {
            if byte == b'\n' || column >= WIDTH {
                break;
            }
            cells[column] = GREEN_ON_BLACK | u16::from(byte);
            column += 1;
        }
    }
}

pub fn write_line(text: &[u8]) {
    unsafe {
        let cells = core::slice::from_raw_parts_mut(BUFFER, WIDTH);
        for cell in cells.iter_mut() {
            *cell = GREEN_ON_BLACK | u16::from(b' ');
        }
        for (column, &byte) in text.iter().take(WIDTH).enumerate() {
            cells[column] = GREEN_ON_BLACK | u16::from(byte);
        }
    }
}
