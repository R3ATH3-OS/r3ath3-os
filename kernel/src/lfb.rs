//! Text console on a linear framebuffer.
//!
//! # Invariants
//! Used only after a Multiboot2 boot that supplied an RGB framebuffer at
//! 32 bits per pixel, at least 640×400. The 80×25 console is 8×16 cells
//! (640×400), centered. Green is `0x55FF55`, black is zero. The cursor is
//! an underline on the bottom row of the cell. Scrolling copies pixel rows.
//! `vga`'s public console calls come here when `is_active` is set. One CPU,
//! main context only, the same rule as `vga`. PVH boots never call `init`
//! successfully, so `is_active` stays false and `vga` keeps `0xB8000`.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::font8x16;
use crate::mb2;
use crate::paging;

const WIDTH: usize = 80;
const HEIGHT: usize = 25;
const CELL_W: usize = 8;
const CELL_H: usize = 16;
const CONSOLE_W: usize = WIDTH * CELL_W;
const CONSOLE_H: usize = HEIGHT * CELL_H;
const GIB: u64 = 1 << 30;
const GREEN_R: u8 = 0x55;
const GREEN_G: u8 = 0xFF;
const GREEN_B: u8 = 0x55;

struct Geom {
    base: u64,
    pitch: usize,
    screen_w: usize,
    screen_h: usize,
    origin_x: usize,
    origin_y: usize,
    bytes: u64,
    red_pos: u8,
    red_size: u8,
    green_pos: u8,
    green_size: u8,
    blue_pos: u8,
    blue_size: u8,
    row: usize,
    col: usize,
    scrolls: u32,
    grid: [u8; WIDTH * HEIGHT],
}

struct Cell(UnsafeCell<Geom>);

// SAFETY: one CPU, main context only. IRQ handlers never enter this module.
// `init` publishes `ACTIVE` only after `GEOM` is fully written.
unsafe impl Sync for Cell {}

static GEOM: Cell = Cell(UnsafeCell::new(Geom {
    base: 0,
    pitch: 0,
    screen_w: 0,
    screen_h: 0,
    origin_x: 0,
    origin_y: 0,
    bytes: 0,
    red_pos: 0,
    red_size: 0,
    green_pos: 0,
    green_size: 0,
    blue_pos: 0,
    blue_size: 0,
    row: 0,
    col: 0,
    scrolls: 0,
    grid: [0; WIDTH * HEIGHT],
}));

static ACTIVE: AtomicBool = AtomicBool::new(false);

fn with_geom<R>(f: impl FnOnce(&mut Geom) -> R) -> R {
    // SAFETY: one CPU; only the shell task (or a boot proof) calls the
    // console; IRQ handlers never do. The `&mut` does not escape `f`.
    f(unsafe { &mut *GEOM.0.get() })
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Install the console when the firmware gave a 32 bpp RGB framebuffer big
/// enough for 80×25. A text-mode tag leaves `vga` in place.
pub fn init() {
    if is_active() {
        return;
    }
    let Some(fb) = mb2::framebuffer() else {
        return;
    };
    if fb.kind != 1 || fb.bpp != 32 {
        return;
    }
    if fb.width < CONSOLE_W as u32 || fb.height < CONSOLE_H as u32 {
        return;
    }
    if fb.pitch < fb.width.saturating_mul(4) || fb.pitch % 4 != 0 || fb.addr % 4 != 0 {
        return;
    }
    let bytes = u64::from(fb.pitch).saturating_mul(u64::from(fb.height));
    if bytes == 0 {
        return;
    }
    if fb.addr >= GIB && !paging::map_uncached_2m(fb.addr, bytes) {
        return;
    }
    if fb.addr < GIB && fb.addr.saturating_add(bytes) > GIB {
        return;
    }
    with_geom(|geom| {
        geom.base = fb.addr;
        geom.pitch = fb.pitch as usize;
        geom.screen_w = fb.width as usize;
        geom.screen_h = fb.height as usize;
        geom.origin_x = (fb.width as usize - CONSOLE_W) / 2;
        geom.origin_y = (fb.height as usize - CONSOLE_H) / 2;
        geom.bytes = bytes;
        geom.red_pos = fb.red_pos;
        geom.red_size = fb.red_size;
        geom.green_pos = fb.green_pos;
        geom.green_size = fb.green_size;
        geom.blue_pos = fb.blue_pos;
        geom.blue_size = fb.blue_size;
        geom.row = 0;
        geom.col = 0;
        geom.scrolls = 0;
        geom.grid = [0; WIDTH * HEIGHT];
    });
    ACTIVE.store(true, Ordering::Release);
}

pub struct Screen {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}

pub fn screen() -> Option<Screen> {
    if !is_active() {
        return None;
    }
    Some(with_geom(|geom| Screen {
        addr: geom.base,
        pitch: geom.pitch as u32,
        width: geom.screen_w as u32,
        height: geom.screen_h as u32,
    }))
}

pub fn scrolls() -> u32 {
    if !is_active() {
        return 0;
    }
    with_geom(|geom| geom.scrolls)
}

pub fn clear() {
    if !is_active() {
        return;
    }
    with_geom(|geom| {
        clear_screen(geom);
        geom.grid = [b' '; WIDTH * HEIGHT];
        geom.row = 0;
        geom.col = 0;
        geom.scrolls = 0;
        draw_cursor(geom);
    });
}

pub fn put(byte: u8) {
    if !is_active() {
        return;
    }
    with_geom(|geom| {
        if byte == b'\n' {
            newline(geom);
        } else {
            if geom.col == WIDTH {
                newline(geom);
            }
            let glyph = if (0x20..=0x7E).contains(&byte) {
                byte
            } else {
                b'?'
            };
            store(geom, geom.row, geom.col, glyph);
            geom.col += 1;
        }
        draw_cursor(geom);
    });
}

pub fn backspace() {
    if !is_active() {
        return;
    }
    with_geom(|geom| {
        if geom.col > 0 {
            geom.col -= 1;
            store(geom, geom.row, geom.col, b' ');
        }
        draw_cursor(geom);
    });
}

pub fn present(grid: &[u8]) {
    if !is_active() {
        return;
    }
    with_geom(|geom| {
        clear_screen(geom);
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
                store(geom, row, col, glyph);
            }
        }
        geom.row = 0;
        geom.col = 0;
        draw_cursor(geom);
    });
}

pub fn write_marker(text: &str) {
    clear();
    for byte in text.bytes() {
        if byte == b'\n' {
            break;
        }
        put(byte);
    }
}

pub fn write_line(text: &[u8]) {
    if !is_active() {
        return;
    }
    with_geom(|geom| {
        for col in 0..WIDTH {
            let glyph = text.get(col).copied().unwrap_or(b' ');
            let glyph = if (0x20..=0x7E).contains(&glyph) {
                glyph
            } else if glyph == 0 {
                b' '
            } else {
                b'?'
            };
            store(geom, 0, col, glyph);
        }
        geom.row = 0;
        geom.col = 0;
        draw_cursor(geom);
    });
}

/// FNV-1a 64 over the 640×400 console rectangle, 4 bytes per pixel, in the
/// order they sit in memory.
pub fn console_fnv() -> u64 {
    if !is_active() {
        return 0;
    }
    with_geom(|geom| {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for y in 0..CONSOLE_H {
            for x in 0..CONSOLE_W {
                let pixel = read_pixel(geom, geom.origin_x + x, geom.origin_y + y);
                for shift in [0u32, 8, 16, 24] {
                    hash ^= u64::from((pixel >> shift) & 0xFF);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        hash
    })
}

/// Every lit pixel of the cell matches the font, and the cell is not the cursor.
pub fn glyph_matches(row: usize, col: usize) -> bool {
    if !is_active() || row >= HEIGHT || col >= WIDTH {
        return false;
    }
    with_geom(|geom| {
        if row == geom.row && col == geom.col.min(WIDTH - 1) && geom.col < WIDTH {
            return false;
        }
        let glyph = geom.grid[row * WIDTH + col];
        cell_matches_font(geom, row, col, glyph, false)
    })
}

fn store(geom: &mut Geom, row: usize, col: usize, glyph: u8) {
    geom.grid[row * WIDTH + col] = glyph;
    blit(geom, row, col, glyph, false);
}

fn newline(geom: &mut Geom) {
    erase_underline(geom);
    geom.col = 0;
    if geom.row + 1 == HEIGHT {
        scroll(geom);
    } else {
        geom.row += 1;
    }
}

fn scroll(geom: &mut Geom) {
    geom.grid.copy_within(WIDTH.., 0);
    for col in 0..WIDTH {
        geom.grid[(HEIGHT - 1) * WIDTH + col] = b' ';
    }
    let row_bytes = CONSOLE_W * 4;
    for y in 0..(CONSOLE_H - CELL_H) {
        for x in 0..row_bytes {
            let src = pixel_ptr(geom, geom.origin_x * 4 + x, geom.origin_y + y + CELL_H);
            let dst = pixel_ptr(geom, geom.origin_x * 4 + x, geom.origin_y + y);
            // SAFETY: both pointers are inside the framebuffer bytes checked
            // in `init`. One CPU. The copy moves the console up one cell.
            unsafe {
                dst.write_volatile(src.read_volatile());
            }
        }
    }
    for row in (HEIGHT - 1)..HEIGHT {
        for col in 0..WIDTH {
            blit(geom, row, col, b' ', false);
        }
    }
    geom.scrolls = geom.scrolls.saturating_add(1);
}

fn draw_cursor(geom: &mut Geom) {
    let col = geom.col.min(WIDTH - 1);
    let glyph = geom.grid[geom.row * WIDTH + col];
    blit(geom, geom.row, col, glyph, true);
}

fn erase_underline(geom: &mut Geom) {
    let col = geom.col.min(WIDTH - 1);
    let glyph = geom.grid[geom.row * WIDTH + col];
    blit(geom, geom.row, col, glyph, false);
}

fn blit(geom: &Geom, row: usize, col: usize, glyph: u8, cursor: bool) {
    let green = pack(geom, GREEN_R, GREEN_G, GREEN_B);
    for gy in 0..CELL_H {
        let mut bits = font8x16::GLYPHS[glyph as usize][gy];
        if cursor && gy + 1 == CELL_H {
            bits = 0xFF;
        }
        for gx in 0..CELL_W {
            let on = bits & (0x80 >> gx) != 0;
            let color = if on { green } else { 0 };
            write_pixel(
                geom,
                geom.origin_x + col * CELL_W + gx,
                geom.origin_y + row * CELL_H + gy,
                color,
            );
        }
    }
}

fn cell_matches_font(geom: &Geom, row: usize, col: usize, glyph: u8, cursor: bool) -> bool {
    let green = pack(geom, GREEN_R, GREEN_G, GREEN_B);
    for gy in 0..CELL_H {
        let mut bits = font8x16::GLYPHS[glyph as usize][gy];
        if cursor && gy + 1 == CELL_H {
            bits = 0xFF;
        }
        for gx in 0..CELL_W {
            let on = bits & (0x80 >> gx) != 0;
            let want = if on { green } else { 0 };
            let got = read_pixel(
                geom,
                geom.origin_x + col * CELL_W + gx,
                geom.origin_y + row * CELL_H + gy,
            );
            if got != want {
                return false;
            }
        }
    }
    true
}

fn clear_screen(geom: &Geom) {
    let pixels = geom.bytes / 4;
    let base = geom.base as *mut u32;
    for index in 0..pixels as usize {
        // SAFETY: `index` is below the framebuffer byte count checked in
        // `init`, and that range is mapped. One CPU. The fill is black.
        unsafe {
            base.add(index).write_volatile(0);
        }
    }
}

fn pack(geom: &Geom, red: u8, green: u8, blue: u8) -> u32 {
    channel(red, geom.red_pos, geom.red_size)
        | channel(green, geom.green_pos, geom.green_size)
        | channel(blue, geom.blue_pos, geom.blue_size)
}

fn channel(value: u8, pos: u8, size: u8) -> u32 {
    if size == 0 || size > 8 || pos >= 32 {
        return 0;
    }
    let shifted = u32::from(value) >> (8 - size);
    let mask = (1u32 << size) - 1;
    (shifted & mask) << pos
}

fn pixel_ptr(geom: &Geom, byte_x: usize, y: usize) -> *mut u8 {
    (geom.base as usize + y * geom.pitch + byte_x) as *mut u8
}

fn write_pixel(geom: &Geom, x: usize, y: usize, color: u32) {
    if x >= geom.screen_w || y >= geom.screen_h {
        return;
    }
    let ptr = pixel_ptr(geom, x * 4, y) as *mut u32;
    // SAFETY: `x` and `y` are inside the framebuffer, which `init` mapped.
    // The address is 4-byte aligned because `x * 4` and a 4-aligned base
    // (firmware framebuffers are) keep it aligned; the write is volatile.
    unsafe {
        ptr.write_volatile(color);
    }
}

fn read_pixel(geom: &Geom, x: usize, y: usize) -> u32 {
    if x >= geom.screen_w || y >= geom.screen_h {
        return 0;
    }
    let ptr = pixel_ptr(geom, x * 4, y) as *const u32;
    // SAFETY: as `write_pixel`. The read sees the bytes just written.
    unsafe { ptr.read_volatile() }
}
