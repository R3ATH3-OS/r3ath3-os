//! Glue Duty rules. Integers only. See `docs/WASM.md` section 8.
#![forbid(unsafe_code)]

pub const TITLE: u8 = 0;
pub const PLAYING: u8 = 1;
pub const PAUSED: u8 = 2;
pub const SHIFT_DONE: u8 = 3;
pub const GAME_OVER: u8 = 4;

const BELT_COLS: i32 = 72;
const SCRAPER_COL: i32 = 8;
const SHIFT_STEPS: u32 = 750;
const MAX_BLOBS: usize = 24;

#[derive(Clone, Copy)]
pub(crate) struct Blob {
    pub(crate) lane: u8,
    pub(crate) r: u16,
    pub(crate) hp: i32,
    pub(crate) x16: i32,
}

pub struct Game {
    pub(crate) rng: u32,
    pub(crate) state: u8,
    pub(crate) shift: u32,
    pub(crate) elapsed: u32,
    pub(crate) score: u32,
    pub(crate) cleaned: u32,
    pub(crate) solvent: u32,
    pub(crate) combo: u32,
    pub(crate) combo_timer: u32,
    pub(crate) jam: u32,
    since_jam: u32,
    pub(crate) lane: u8,
    last_dir: u8,
    stroke_age: u32,
    spawn_wait: u32,
    pub(crate) blob_count: u8,
    pub(crate) blobs: [Blob; MAX_BLOBS],
    pub(crate) log: [u8; 72],
    pub(crate) log_len: u8,
    pub(crate) grid: [u8; 2000],
    pub(crate) summary: [u8; 61],
}

impl Game {
    pub const fn fresh() -> Game {
        Game {
            rng: 1,
            state: TITLE,
            shift: 1,
            elapsed: 0,
            score: 0,
            cleaned: 0,
            solvent: 2,
            combo: 0,
            combo_timer: 0,
            jam: 0,
            since_jam: 0,
            lane: 1,
            last_dir: 0,
            stroke_age: 0,
            spawn_wait: 0,
            blob_count: 0,
            blobs: [Blob {
                lane: 0,
                r: 0,
                hp: 0,
                x16: 0,
            }; MAX_BLOBS],
            log: [0; 72],
            log_len: 0,
            grid: [0; 2000],
            summary: [0; 61],
        }
    }

    /// Seed 0 becomes `0x9E3779B9`. Leaves the game on the title screen.
    pub fn init(&mut self, seed: u32) {
        self.reset(if seed == 0 { 0x9E37_79B9 } else { seed });
        self.set_log(b"Nothing yet. The belts are suspiciously clean.");
        super::text::paint(self);
    }

    pub fn key(&mut self, key: u8) {
        match self.state {
            TITLE => {
                if key == b' ' {
                    self.state = PLAYING;
                    self.arm_spawn();
                }
            }
            PLAYING => match key {
                b'w' => {
                    if self.lane > 0 {
                        self.lane -= 1;
                    }
                }
                b's' => {
                    if self.lane < 2 {
                        self.lane += 1;
                    }
                }
                b'a' => self.stroke(1),
                b'd' => self.stroke(2),
                b' ' => self.solvent(),
                b'p' => self.state = PAUSED,
                _ => {}
            },
            PAUSED => {
                if key == b'p' {
                    self.state = PLAYING;
                }
            }
            SHIFT_DONE => {
                if key == b' ' {
                    self.next_shift();
                }
            }
            GAME_OVER => {
                if key == b' ' {
                    self.new_game();
                }
            }
            _ => {}
        }
        super::text::paint(self);
    }

    pub fn step(&mut self) -> u8 {
        if self.state == PLAYING {
            self.tick_play();
        }
        super::text::paint(self);
        self.state
    }

    pub fn state(&self) -> u8 {
        self.state
    }

    pub fn grid(&self) -> &[u8; 2000] {
        &self.grid
    }

    /// Length byte followed by that many summary bytes.
    pub fn summary(&self) -> &[u8] {
        let len = self.summary[0] as usize;
        let end = 1 + len;
        if end <= self.summary.len() {
            &self.summary[..end]
        } else {
            &self.summary[..1]
        }
    }

    fn reset(&mut self, rng: u32) {
        self.rng = rng;
        self.state = TITLE;
        self.shift = 1;
        self.elapsed = 0;
        self.score = 0;
        self.cleaned = 0;
        self.solvent = 2;
        self.combo = 0;
        self.combo_timer = 0;
        self.jam = 0;
        self.since_jam = 0;
        self.lane = 1;
        self.last_dir = 0;
        self.stroke_age = 0;
        self.spawn_wait = 0;
        self.blob_count = 0;
    }

    fn new_game(&mut self) {
        let rng = self.rng;
        self.reset(rng);
        self.state = PLAYING;
        self.set_log(b"New shift. The belt is clear.");
        self.arm_spawn();
    }

    fn next_shift(&mut self) {
        self.shift = self.shift.saturating_add(1);
        self.jam = self.jam.saturating_sub(2500);
        self.blob_count = 0;
        self.elapsed = 0;
        self.state = PLAYING;
        self.set_log(b"Next shift. The belt is moving again.");
        self.arm_spawn();
    }

    fn arm_spawn(&mut self) {
        self.spawn_wait = self.spawn_interval();
    }

    fn tick_play(&mut self) {
        if self.spawn_wait > 0 {
            self.spawn_wait -= 1;
        }
        if self.spawn_wait == 0 {
            self.try_spawn();
            self.spawn_wait = self.spawn_interval();
        }
        let speed = 4 + self.shift as i32 + (self.elapsed / 150) as i32;
        let mut i = 0;
        while i < self.blob_count as usize {
            self.blobs[i].x16 += speed;
            i += 1;
        }
        self.resolve_escapes();
        if self.since_jam > 40 {
            self.jam = self.jam.saturating_sub(16);
        }
        self.since_jam = self.since_jam.saturating_add(1);
        if self.combo_timer > 0 {
            self.combo_timer -= 1;
            if self.combo_timer == 0 {
                self.combo = 0;
            }
        }
        self.stroke_age = self.stroke_age.saturating_add(1);
        self.elapsed = self.elapsed.saturating_add(1);
        if self.jam >= 10_000 {
            self.jam = 10_000;
            self.state = GAME_OVER;
            self.set_log(b"The belt gummed up.");
        } else if self.elapsed >= SHIFT_STEPS {
            let bonus = (10_000 - self.jam) * 5 / 100;
            self.score = self.score.wrapping_add(bonus);
            self.state = SHIFT_DONE;
            self.set_log(b"Shift complete.");
        }
    }

    fn try_spawn(&mut self) {
        if self.blob_count as usize >= MAX_BLOBS {
            return;
        }
        let lane = self.rand(3) as u8;
        let mut r = 13 + self.rand(23);
        if self.rand(100) < 14 {
            r += 10;
        }
        let r = r as u16;
        let hp = (u32::from(r) * 12 / 5) as i32;
        let index = self.blob_count as usize;
        self.blobs[index] = Blob {
            lane,
            r,
            hp,
            x16: 0,
        };
        self.blob_count += 1;
    }

    fn rand(&mut self, n: u32) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x % n
    }

    fn spawn_interval(&mut self) -> u32 {
        let raw = 115 - 7 * self.shift as i32 - (self.elapsed as i32) * 6 / 100;
        let base = if raw < 42 { 42 } else { raw as u32 };
        let roll = 70 + self.rand(61);
        let steps = base * roll / 1000;
        if steps < 1 {
            1
        } else {
            steps
        }
    }

    fn resolve_escapes(&mut self) {
        let mut i = 0;
        while i < self.blob_count as usize {
            let blob = self.blobs[i];
            let left = blob.x16 / 16;
            if left + width(blob.r) > BELT_COLS {
                let r = blob.r;
                self.remove_at(i);
                let add = 900 + 35 * u32::from(r);
                self.jam = self.jam.saturating_add(add);
                if self.jam > 10_000 {
                    self.jam = 10_000;
                }
                self.combo = 0;
                self.combo_timer = 0;
                self.since_jam = 0;
                self.set_log(b"A blob reached the discharge.");
            } else {
                i += 1;
            }
        }
    }

    fn stroke(&mut self, dir: u8) {
        let bonus = self.last_dir != 0 && self.last_dir != dir && self.stroke_age <= 3;
        let dmg = if bonus { 25 } else { 10 };
        self.last_dir = dir;
        self.stroke_age = 0;
        let Some(index) = self.nearest_overlap() else {
            return;
        };
        self.blobs[index].hp -= dmg;
        if self.blobs[index].hp <= 0 {
            let r = self.blobs[index].r;
            self.remove_at(index);
            self.score_clean(r);
            self.set_log(b"Scrubbed a blob.");
        }
    }

    fn nearest_overlap(&self) -> Option<usize> {
        let mut best: Option<(i32, usize)> = None;
        let mut i = 0;
        while i < self.blob_count as usize {
            let blob = self.blobs[i];
            if blob.lane == self.lane {
                let left = blob.x16 / 16;
                let right = left + width(blob.r);
                if left <= SCRAPER_COL && SCRAPER_COL < right {
                    let dist = (left - SCRAPER_COL).abs();
                    let take = match best {
                        Some((prev, _)) => dist < prev,
                        None => true,
                    };
                    if take {
                        best = Some((dist, i));
                    }
                }
            }
            i += 1;
        }
        best.map(|(_, index)| index)
    }

    fn solvent(&mut self) {
        if self.solvent == 0 {
            return;
        }
        self.solvent -= 1;
        let lo_lane = self.lane.saturating_sub(1);
        let hi_lane = if self.lane >= 2 { 2 } else { self.lane + 1 };
        let lo = SCRAPER_COL - 10;
        let hi = SCRAPER_COL + 10;
        let mut hit = [false; MAX_BLOBS];
        let mut any = false;
        let mut i = 0;
        while i < self.blob_count as usize {
            let blob = self.blobs[i];
            if blob.lane >= lo_lane && blob.lane <= hi_lane {
                let left = blob.x16 / 16;
                let right = left + width(blob.r);
                if left <= hi && right > lo {
                    hit[i] = true;
                    any = true;
                }
            }
            i += 1;
        }
        i = 0;
        while i < self.blob_count as usize {
            if hit[i] {
                self.score_clean(self.blobs[i].r);
            }
            i += 1;
        }
        self.retain_unhit(&hit);
        if any {
            self.set_log(b"Solvent blast.");
        }
    }

    fn retain_unhit(&mut self, hit: &[bool; MAX_BLOBS]) {
        let mut dst = 0;
        let mut i = 0;
        let count = self.blob_count as usize;
        while i < count {
            if !hit[i] {
                self.blobs[dst] = self.blobs[i];
                dst += 1;
            }
            i += 1;
        }
        self.blob_count = dst as u8;
    }

    fn score_clean(&mut self, r: u16) {
        let mult = 1 + (self.combo / 3).min(4);
        let add = (40 + 3 * u32::from(r)) * mult;
        self.score = self.score.wrapping_add(add);
        self.combo = self.combo.saturating_add(1);
        self.combo_timer = 26;
        self.cleaned = self.cleaned.saturating_add(1);
        if self.cleaned % 12 == 0 {
            self.solvent = self.solvent.saturating_add(1);
        }
    }

    fn remove_at(&mut self, index: usize) {
        let count = self.blob_count as usize;
        let mut i = index + 1;
        while i < count {
            self.blobs[i - 1] = self.blobs[i];
            i += 1;
        }
        if self.blob_count > 0 {
            self.blob_count -= 1;
        }
    }

    pub(crate) fn set_log(&mut self, text: &[u8]) {
        let n = text.len().min(self.log.len());
        let mut i = 0;
        while i < n {
            super::text::store_byte(&mut self.log[i], text[i]);
            i += 1;
        }
        self.log_len = n as u8;
    }
}

fn width(r: u16) -> i32 {
    1 + i32::from(r) / 16
}

/// FNV-1a 64. A checksum for comparing two grids, not a security check.
pub fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}
