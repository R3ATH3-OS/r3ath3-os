//! 80×25 text grid for Glue Duty. One byte per cell. See `docs/WASM.md`.
#![forbid(unsafe_code)]

use super::logic::{Game, GAME_OVER, PAUSED, PLAYING, SHIFT_DONE, TITLE};

/// One store the compiler must not turn into a bulk-memory fill or copy.
#[inline(never)]
pub(crate) fn store_byte(slot: &mut u8, byte: u8) {
    *slot = byte;
}

pub fn paint(game: &mut Game) {
    let mut i = 0;
    while i < game.grid.len() {
        store_byte(&mut game.grid[i], b' ');
        i += 1;
    }
    paint_hud(game);
    paint_belt(game);
    paint_middle(game);
    paint_log(game);
    paint_help(game);
    paint_summary(game);
    super::pixels::sync(game);
}

fn paint_hud(game: &mut Game) {
    let shift = game.shift;
    let elapsed = game.elapsed;
    let score = game.score;
    let cleaned = game.cleaned;
    let solvent = game.solvent;
    let combo = game.combo;
    let jam = game.jam;
    {
        let mut pen = Pen::new(&mut game.grid, 0);
        pen.str(b"GLUE DUTY  shift ");
        pen.u32(shift);
        pen.str(b"  time ");
        let (mins, secs) = clock(elapsed);
        pen.u32(mins);
        pen.ch(b':');
        pen.ch(b'0' + (secs / 10) as u8);
        pen.ch(b'0' + (secs % 10) as u8);
        pen.str(b"  score ");
        pen.u32(score);
        pen.str(b"  cleaned ");
        pen.u32(cleaned);
        pen.str(b"  solvent ");
        pen.u32(solvent);
        pen.str(b"  combo x");
        let mult = 1 + (combo / 3).min(4);
        pen.u32(mult);
    }
    let mut bar = Pen::new(&mut game.grid, 1);
    bar.str(b"gum-up [");
    let percent = (jam / 100).min(100);
    let hashes = (percent / 5).min(20) as usize;
    let mut cell = 0;
    while cell < 20 {
        bar.ch(if cell < hashes { b'#' } else { b'.' });
        cell += 1;
    }
    bar.str(b"] ");
    bar.u32_width(percent, 3);
    bar.ch(b'%');
}

fn clock(elapsed: u32) -> (u32, u32) {
    let left = 750u32.saturating_sub(elapsed.min(750));
    let secs = if left == 0 { 0 } else { (left + 9) / 10 };
    (secs / 60, secs % 60)
}

fn paint_belt(game: &mut Game) {
    let scraper = game.lane;
    let count = game.blob_count as usize;
    let rails = [4usize, 6, 8, 10];
    let mut r = 0;
    while r < rails.len() {
        let row = rails[r];
        let mut col = 0;
        while col < 4 {
            put(&mut game.grid, row, col, b' ');
            col += 1;
        }
        while col < 68 {
            put(&mut game.grid, row, col, b'=');
            col += 1;
        }
        while col < 76 {
            put(&mut game.grid, row, col, b'!');
            col += 1;
        }
        r += 1;
    }
    let lanes = [5usize, 7, 9];
    let mut lane = 0;
    while lane < 3 {
        let row = lanes[lane];
        put(&mut game.grid, row, 0, b' ');
        put(&mut game.grid, row, 1, b'1' + lane as u8);
        put(&mut game.grid, row, 2, b' ');
        put(&mut game.grid, row, 3, b' ');
        let mut col = 4;
        while col < 68 {
            put(&mut game.grid, row, col, b'-');
            col += 1;
        }
        while col < 76 {
            put(&mut game.grid, row, col, b':');
            col += 1;
        }
        lane += 1;
    }
    let mut index = 0;
    while index < count {
        let blob = game.blobs[index];
        let left = blob.x16 / 16;
        let w = 1 + i32::from(blob.r) / 16;
        let glyph = if blob.r < 20 {
            b'o'
        } else if blob.r < 35 {
            b'O'
        } else {
            b'@'
        };
        let row = 5 + blob.lane as usize * 2;
        let mut cell = 0;
        while cell < w {
            let belt = left + cell;
            if belt >= 0 && belt < 72 {
                put(&mut game.grid, row, 4 + belt as usize, glyph);
            }
            cell += 1;
        }
        index += 1;
    }
    let rail = match scraper {
        0 => 4,
        1 => 6,
        _ => 8,
    };
    put(&mut game.grid, rail, 12, b'v');
}

fn paint_middle(game: &mut Game) {
    let state = game.state;
    match state {
        TITLE => {
            row(&mut game.grid, 12, b"Three lanes. Scrub glue, before it ships!");
            row(
                &mut game.grid,
                13,
                b"Scrub with a and d. Alternate strokes cut deeper. Space is solvent.",
            );
            row(
                &mut game.grid,
                14,
                b"Keep the discharge clear. A blob that slips through gums the whole belt.",
            );
            row(
                &mut game.grid,
                15,
                b"Survive the shift. The next one is faster and cleaner.",
            );
            row(&mut game.grid, 16, b"Press space to start work.");
        }
        PAUSED => row(&mut game.grid, 12, b"PAUSED  p: resume"),
        SHIFT_DONE => row(&mut game.grid, 12, b"SHIFT DONE  space: next shift"),
        GAME_OVER => row(&mut game.grid, 12, b"GUMMED UP  space: new game"),
        PLAYING => {}
        _ => {}
    }
    if state == TITLE {
        let mut pen = Pen::new(&mut game.grid, 2);
        pen.str(b"    scrub before the glue ships");
        while pen.col < 64 {
            pen.ch(b' ');
        }
        pen.str(b"DISCHARGE ->");
    } else {
        let mut pen = Pen::new(&mut game.grid, 2);
        while pen.col < 64 {
            pen.ch(b' ');
        }
        pen.str(b"DISCHARGE ->");
    }
}

fn paint_log(game: &mut Game) {
    let len = game.log_len as usize;
    let mut pen = Pen::new(&mut game.grid, 19);
    pen.str(b"  log: ");
    let mut i = 0;
    while i < len && i < game.log.len() {
        let byte = game.log[i];
        pen.ch(byte);
        i += 1;
    }
}

fn paint_help(game: &mut Game) {
    row(
        &mut game.grid,
        24,
        b"w s: lane   a d: scrub (alternate)   space: solvent   p: pause   q: quit",
    );
}

fn paint_summary(game: &mut Game) {
    let mut body = [0u8; 60];
    let mut n = 0;
    n = push(&mut body, n, b"shift ");
    n = push_u32(&mut body, n, game.shift);
    n = push(&mut body, n, b" score ");
    n = push_u32(&mut body, n, game.score);
    n = push(&mut body, n, b" cleaned ");
    n = push_u32(&mut body, n, game.cleaned);
    n = push(&mut body, n, b" gum-up ");
    n = push_u32(&mut body, n, game.jam / 100);
    n = push(&mut body, n, b"%");
    store_byte(&mut game.summary[0], n as u8);
    let mut i = 0;
    while i < n {
        store_byte(&mut game.summary[1 + i], body[i]);
        i += 1;
    }
}

fn push(buf: &mut [u8], mut n: usize, text: &[u8]) -> usize {
    let mut i = 0;
    while i < text.len() && n < buf.len() {
        buf[n] = text[i];
        n += 1;
        i += 1;
    }
    n
}

fn push_u32(buf: &mut [u8], mut n: usize, mut value: u32) -> usize {
    let mut tmp = [0u8; 10];
    let mut i = 0;
    if value == 0 {
        if n < buf.len() {
            buf[n] = b'0';
            n += 1;
        }
        return n;
    }
    while value > 0 && i < tmp.len() {
        tmp[i] = b'0' + (value % 10) as u8;
        value /= 10;
        i += 1;
    }
    while i > 0 && n < buf.len() {
        i -= 1;
        buf[n] = tmp[i];
        n += 1;
    }
    n
}

fn row(grid: &mut [u8; 2000], at: usize, text: &[u8]) {
    let mut pen = Pen::new(grid, at);
    pen.str(text);
}

fn put(grid: &mut [u8; 2000], row: usize, col: usize, byte: u8) {
    if row < 25 && col < 80 {
        store_byte(&mut grid[row * 80 + col], byte);
    }
}

struct Pen<'a> {
    grid: &'a mut [u8; 2000],
    row: usize,
    col: usize,
}

impl<'a> Pen<'a> {
    fn new(grid: &'a mut [u8; 2000], row: usize) -> Pen<'a> {
        Pen { grid, row, col: 0 }
    }

    fn ch(&mut self, byte: u8) {
        if self.col < 80 {
            put(self.grid, self.row, self.col, byte);
            self.col += 1;
        }
    }

    fn str(&mut self, text: &[u8]) {
        let mut i = 0;
        while i < text.len() {
            self.ch(text[i]);
            i += 1;
        }
    }

    fn u32(&mut self, value: u32) {
        let mut tmp = [0u8; 10];
        let mut n = 0;
        let mut value = value;
        if value == 0 {
            self.ch(b'0');
            return;
        }
        while value > 0 && n < tmp.len() {
            tmp[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.ch(tmp[n]);
        }
    }

    fn u32_width(&mut self, value: u32, width: usize) {
        let mut tmp = [0u8; 10];
        let mut n = 0;
        let mut value = value;
        if value == 0 {
            tmp[0] = b'0';
            n = 1;
        } else {
            while value > 0 && n < tmp.len() {
                tmp[n] = b'0' + (value % 10) as u8;
                value /= 10;
                n += 1;
            }
        }
        let mut pad = width.saturating_sub(n);
        while pad > 0 {
            self.ch(b' ');
            pad -= 1;
        }
        while n > 0 {
            n -= 1;
            self.ch(tmp[n]);
        }
    }
}
