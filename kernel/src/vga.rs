//! Direct write to the VGA text buffer at physical `0xB8000`.
//!
//! # Invariants
//! Boot code identity-maps the first 1 GiB before `kernel_main`, so this
//! physical address is the same virtual address. Only the boot processor
//! writes it, interrupts are off, and the write covers exactly 80×25 cells.
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
