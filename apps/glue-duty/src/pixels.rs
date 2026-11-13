//! 320×200 picture of the Glue Duty text grid.
//!
//! Each of the 80×25 cells is a 4×8 block. The glyph is a 3×5 bitmap in
//! columns 0–2 and rows 0–4. Column 3, row 7 of a non-space cell is the
//! cell byte itself, so two different grid bytes cannot paint the same
//! block. Space paints nothing. A later paint rewrites only the cells
//! whose byte changed.
//!
//! Palette indexes are `RRRGGGBB` for the mode 13h DAC in `docs/FRAMEBUFFER.md`.
#![forbid(unsafe_code)]

use super::logic::Game;
use super::text::store_byte;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;
pub const LEN: usize = WIDTH * HEIGHT;

const COLS: usize = 80;
const ROWS: usize = 25;
const CELL_W: usize = 4;
const CELL_H: usize = 8;

const _: () = assert!(COLS * CELL_W == WIDTH);
const _: () = assert!(ROWS * CELL_H == HEIGHT);
const _: () = assert!(LEN == 64_000);

/// ASCII 0x20..=0x7E. Bit `row * 3 + col` of a 3×5 glyph. Space is 0.
static FONT: [u16; 95] = [
    0x0000, 0x2092, 0x002d, 0x5f7d, 0x388e, 0x52a5, 0x6aaa, 0x0012, 0x224a, 0x1491, 0x0aa8, 0x05d0,
    0x1400, 0x01c0, 0x2000, 0x12a4, 0x7b6f, 0x749a, 0x73e7, 0x79e7, 0x49ed, 0x79cf, 0x7bcf, 0x2527,
    0x7bef, 0x79ef, 0x0410, 0x1410, 0x4454, 0x0e38, 0x1511, 0x20a7, 0x63ef, 0x5bea, 0x3aeb, 0x624e,
    0x3b6b, 0x72cf, 0x12cf, 0x6b4e, 0x5bed, 0x7497, 0x2b24, 0x5aed, 0x7249, 0x5b7d, 0x5bdd, 0x2b6a,
    0x12eb, 0x4d6a, 0x5aeb, 0x388e, 0x2497, 0x7b6d, 0x2b6d, 0x5f6d, 0x5aad, 0x24ad, 0x72a7, 0x324b,
    0x4889, 0x6926, 0x002a, 0x7000, 0x0011, 0x6b70, 0x3b59, 0x6270, 0x6b74, 0x63f0, 0x12ce, 0x4d70,
    0x5b59, 0x2482, 0x1482, 0x5ae9, 0x2492, 0x5bd8, 0x5b58, 0x2b50, 0x1758, 0x4d70, 0x12e8, 0x38b0,
    0x24ba, 0x6b68, 0x2b68, 0x5f68, 0x5aa8, 0x1d68, 0x72b8, 0x6456, 0x2492, 0x3513, 0x00f0,
];

/// Paint `game.pixels` from `game.grid`. Cells that still match `game.shown`
/// are left alone.
pub fn sync(game: &mut Game) {
    let mut i = 0;
    while i < game.grid.len() {
        let next = game.grid[i];
        let prev = game.shown[i];
        if prev != next {
            let col = i % COLS;
            let row = i / COLS;
            blit_cell(game, col, row, prev, true);
            blit_cell(game, col, row, next, false);
            store_byte(&mut game.shown[i], next);
        }
        i += 1;
    }
}

/// One cell. `inline(never)` keeps the stores inside this function, so a
/// full-buffer fill cannot become a `memory.fill`.
#[inline(never)]
fn blit_cell(game: &mut Game, col: usize, row: usize, ch: u8, erase: bool) {
    if ch == 0 || ch == b' ' || row >= ROWS || col >= COLS {
        return;
    }
    let color = if erase { 0 } else { ink(ch) };
    let bits = glyph(ch);
    let x0 = col * CELL_W;
    let y0 = row * CELL_H;
    let mut bit = 0;
    while bit < 15 {
        if bits & (1u16 << bit) != 0 {
            put(game, x0 + (bit % 3), y0 + (bit / 3), color);
        }
        bit += 1;
    }
    put(game, x0 + 3, y0 + 7, if erase { 0 } else { ch });
}

fn put(game: &mut Game, x: usize, y: usize, color: u8) {
    if x < WIDTH && y < HEIGHT {
        // Inside `blit_cell`, which is `inline(never)`, so these stores stay
        // a per-cell loop. They are not a fill of the whole buffer.
        game.pixels[y * WIDTH + x] = color;
    }
}

fn glyph(ch: u8) -> u16 {
    if (0x20..=0x7E).contains(&ch) {
        FONT[(ch - 0x20) as usize]
    } else {
        0
    }
}

/// Ink color for the glyph body. The speck at the corner is the byte itself.
fn ink(ch: u8) -> u8 {
    match ch {
        b'=' | b'#' => 0b111_101_00,
        b'-' | b'.' => 0b010_010_01,
        b'!' | b':' => 0b111_000_00,
        b'v' | b'>' => 0b000_111_11,
        b'o' => 0b111_100_00,
        b'O' => 0b111_010_00,
        b'@' => 0b111_000_11,
        b'0'..=b'9' => 255,
        _ => 0b000_111_00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::{fnv64, Game};

    #[test]
    fn different_bytes_paint_different_blocks() {
        let mut seen = [[0u8; 32]; 96];
        let mut n = 0;
        let mut ch = 0x20u8;
        while ch <= 0x7E {
            let mut game = Game::blank();
            sync_one(&mut game, ch);
            let mut block = [0u8; 32];
            let mut i = 0;
            while i < 8 {
                let mut c = 0;
                while c < 4 {
                    block[i * 4 + c] = game.pixels[i * WIDTH + c];
                    c += 1;
                }
                i += 1;
            }
            let mut prev = 0;
            while prev < n {
                assert_ne!(seen[prev], block, "byte {}", ch);
                prev += 1;
            }
            seen[n] = block;
            n += 1;
            ch += 1;
        }
    }

    fn sync_one(game: &mut Game, ch: u8) {
        game.grid[0] = ch;
        game.shown[0] = 0;
        sync(game);
    }

    #[test]
    fn title_grid_matches_the_text_golden() {
        let mut game = Game::fresh();
        game.init(1);
        let golden = include_str!("../golden/app-glue.vga");
        let mut lines = golden.lines();
        for row in 0..25 {
            let line = lines.next().expect("row");
            let mut cells = line.split_whitespace();
            for col in 0..80 {
                let cell = u16::from_str_radix(cells.next().expect("cell"), 16).unwrap();
                assert_eq!(cell >> 8, 0x0A, "attr {row},{col}");
                assert_eq!(cell as u8, game.grid()[row * 80 + col], "glyph {row},{col}");
            }
            assert!(cells.next().is_none());
        }
        assert!(lines.next().is_none());
    }

    #[test]
    fn title_pixels_are_not_blank_and_match_the_golden() {
        let mut game = Game::fresh();
        game.init(1);
        let pixels = game.pixels();
        let sum: u32 = pixels.iter().map(|b| u32::from(*b)).sum();
        assert!(sum > 0);
        let golden = include_bytes!("../golden/app-glue.pix");
        assert_eq!(golden.len(), LEN);
        assert_eq!(golden, pixels.as_slice());
        let title_fnv = fnv64(pixels);
        assert_eq!(title_fnv, 0xfbc1_f073_536d_b373);
        assert_eq!(sum, 660_216);
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[1], 28);
        assert_eq!(pixels[LEN - 1], 0);

        game.key(b' ');
        let state = game.step();
        assert_eq!(state, 1);
        let play = game.pixels();
        let play_sum: u32 = play.iter().map(|b| u32::from(*b)).sum();
        assert_eq!(play_sum, 574_839);
        assert_eq!(fnv64(play), 0xf14e_49b4_7018_956c);
        assert_ne!(fnv64(play), title_fnv);
    }
}
