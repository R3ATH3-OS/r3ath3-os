//! VGA text console at physical `0xB8000`.
//!
//! # Invariants
//! Boot code identity-maps the first 1 GiB before `kernel_main`, so this
//! physical address is the same virtual address. Only the boot processor
//! writes it. The console cursor is touched only by the shell task. IRQ
//! handlers and other tasks do not. A preemption in the middle of a console
//! call is harmless because no other task enters this module.
//! `write_marker` fills 80×25 cells and `write_line` fills the first row.
//! The console API tracks a cursor, scrolls, and programs the hardware cursor
//! through CRTC ports `0x3D4` and `0x3D5`. QEMU's default VGA text mode
//! displays this buffer. With `-vga none`, the writes are discarded.

// Fault builds do not call the marker writer. Non-shell builds do not call
// the console.
#![allow(dead_code)]

use core::cell::UnsafeCell;

use crate::port;

const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const CELLS: usize = WIDTH * HEIGHT;
const BUFFER: *mut u16 = 0xB8000 as *mut u16;
const GREEN_ON_BLACK: u16 = 0x0A00;
const CRTC_INDEX: u16 = 0x3D4;
const CRTC_DATA: u16 = 0x3D5;

struct ConsolePos {
    row: usize,
    col: usize,
}

struct ConsoleCell(UnsafeCell<ConsolePos>);

// SAFETY: single CPU, main context only. IRQ handlers never touch this cell
// or the VGA buffer.
unsafe impl Sync for ConsoleCell {}

static CONSOLE: ConsoleCell = ConsoleCell(UnsafeCell::new(ConsolePos { row: 0, col: 0 }));

fn with_console<R>(f: impl FnOnce(&mut ConsolePos) -> R) -> R {
    // SAFETY: one CPU; only the shell task calls the console; IRQ handlers
    // and other tasks never do; a preemption mid-call is harmless because
    // no other task enters the module. The `&mut` does not escape `f`.
    f(unsafe { &mut *CONSOLE.0.get() })
}

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

/// Paint an 80×25 grid. `0x00` is a space, `0x20`–`0x7E` are themselves, and
/// every other byte is `?`. The attribute is green on black (`0x0A`).
pub fn present(grid: &[u8]) {
    for row in 0..HEIGHT {
        for col in 0..WIDTH {
            let index = row * WIDTH + col;
            let byte = grid.get(index).copied().unwrap_or(0);
            let glyph = if byte == 0x00 {
                b' '
            } else if (0x20..=0x7E).contains(&byte) {
                byte
            } else {
                b'?'
            };
            write_cell(row, col, GREEN_ON_BLACK | u16::from(glyph));
        }
    }
    with_console(|pos| {
        pos.row = 0;
        pos.col = 0;
    });
    set_cursor(0, 0);
}

/// Fill the 80×25 buffer with green-on-black spaces and home the cursor.
pub fn console_clear() {
    for index in 0..CELLS {
        // SAFETY: `index` is below 2000, so the write stays inside the VGA
        // text buffer at 0xB8000. Single CPU, main context only.
        unsafe {
            BUFFER.add(index).write_volatile(space_cell());
        }
    }
    with_console(|pos| {
        pos.row = 0;
        pos.col = 0;
    });
    set_cursor(0, 0);
}

/// Write one byte. `\n` moves to column 0 of the next row. A printable byte
/// (`0x20`–`0x7E`) is stored and `col` advances. Any other byte is stored as
/// `?`. When `col` is already 80, a printable byte first moves to the next
/// row; a `\n` at column 80 moves to the next row once and does not insert a
/// blank line. Moving past row 24 scrolls.
pub fn console_put(byte: u8) {
    let (row, col) = with_console(|pos| {
        if byte == b'\n' {
            newline(pos);
        } else {
            if pos.col == WIDTH {
                newline(pos);
            }
            let glyph = if (0x20..=0x7E).contains(&byte) {
                byte
            } else {
                b'?'
            };
            write_cell(pos.row, pos.col, GREEN_ON_BLACK | u16::from(glyph));
            pos.col += 1;
        }
        (pos.row, pos.col)
    });
    set_cursor(row, col);
}

/// Erase the previous cell on this row. Does nothing at column 0, and never
/// moves to the previous row.
pub fn console_backspace() {
    let (row, col) = with_console(|pos| {
        if pos.col > 0 {
            pos.col -= 1;
            write_cell(pos.row, pos.col, space_cell());
        }
        (pos.row, pos.col)
    });
    set_cursor(row, col);
}

fn space_cell() -> u16 {
    GREEN_ON_BLACK | u16::from(b' ')
}

fn newline(pos: &mut ConsolePos) {
    pos.col = 0;
    if pos.row + 1 == HEIGHT {
        scroll();
    } else {
        pos.row += 1;
    }
}

fn scroll() {
    for row in 0..(HEIGHT - 1) {
        for col in 0..WIDTH {
            let value = read_cell(row + 1, col);
            write_cell(row, col, value);
        }
    }
    for col in 0..WIDTH {
        write_cell(HEIGHT - 1, col, space_cell());
    }
}

fn write_cell(row: usize, col: usize, value: u16) {
    // SAFETY: callers pass `row < 25` and `col < 80`, so the index is inside
    // the 2000-cell VGA text buffer. Single CPU, main context only.
    unsafe {
        BUFFER.add(row * WIDTH + col).write_volatile(value);
    }
}

fn read_cell(row: usize, col: usize) -> u16 {
    // SAFETY: same bounds as `write_cell`. The read is volatile so a later
    // scroll sees the cells just written.
    unsafe { BUFFER.add(row * WIDTH + col).read_volatile() }
}

fn set_cursor(row: usize, col: usize) {
    let pos = (row * WIDTH + col.min(WIDTH - 1)) as u16;
    // SAFETY: 0x3D4 and 0x3D5 are the VGA CRTC index and data ports. Single
    // CPU, main context only. IRQ handlers do not program the cursor.
    unsafe {
        port::outb(CRTC_INDEX, 0x0E);
        port::outb(CRTC_DATA, (pos >> 8) as u8);
        port::outb(CRTC_INDEX, 0x0F);
        port::outb(CRTC_DATA, (pos & 0xFF) as u8);
    }
}
