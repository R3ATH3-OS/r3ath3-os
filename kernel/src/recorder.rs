//! Flight recorder. See `docs/RECORDER.md`.
//!
//! Safe code only. No I/O, no statics, and no allocation.
//! Time is the `tick: u64` argument. This module stores entries. The shell is
//! the only code that appends to the live record or prints one.
//! The record lives in kernel RAM and is lost on reboot.
#![allow(dead_code)]
// each feature uses a subset of this API

pub const TAG_MAX: usize = 24;
pub const BODY_MAX: usize = 76;
pub const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x100_0000_01b3;

/// FNV-1a 64 over the canonical bytes of one entry, starting from `prev`.
pub fn link(prev: u64, n: u32, tick: u64, tag: &[u8], body: &[u8]) -> u64 {
    let mut hash = prev;
    let mut feed = |byte: u8| {
        hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
    };
    for byte in n.to_le_bytes() {
        feed(byte);
    }
    for byte in tick.to_le_bytes() {
        feed(byte);
    }
    feed(tag.len() as u8);
    for &byte in tag {
        feed(byte);
    }
    feed(body.len() as u8);
    for &byte in body {
        feed(byte);
    }
    hash
}

#[derive(Clone, Copy)]
pub struct Entry {
    n: u32,
    tick: u64,
    link: u64,
    tag: [u8; TAG_MAX],
    tag_len: u8,
    body: [u8; BODY_MAX],
    body_len: u8,
}

impl Entry {
    pub const EMPTY: Entry = Entry {
        n: 0,
        tick: 0,
        link: 0,
        tag: [0; TAG_MAX],
        tag_len: 0,
        body: [0; BODY_MAX],
        body_len: 0,
    };

    pub fn n(&self) -> u32 {
        self.n
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn link(&self) -> u64 {
        self.link
    }

    pub fn tag(&self) -> &[u8] {
        &self.tag[..self.tag_len as usize]
    }

    pub fn body(&self) -> &[u8] {
        &self.body[..self.body_len as usize]
    }
}

const _: () = assert!(core::mem::size_of::<Entry>() == 128);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppendError {
    Full,
    Bad,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    Ok,
    Broken { at: u32 },
}

pub struct Recorder<const N: usize> {
    entries: [Entry; N],
    len: usize,
    lost: u32,
}

impl<const N: usize> Recorder<N> {
    pub const fn new() -> Self {
        Self {
            entries: [Entry::EMPTY; N],
            len: 0,
            lost: 0,
        }
    }

    pub fn append(&mut self, tick: u64, tag: &[u8], body: &[u8]) -> Result<u32, AppendError> {
        if !valid_text(tag, 1, TAG_MAX) || !valid_text(body, 0, BODY_MAX) {
            self.note_lost();
            return Err(AppendError::Bad);
        }
        if self.len == N {
            self.note_lost();
            return Err(AppendError::Full);
        }
        let n = (self.len as u32).wrapping_add(1);
        let prev = if self.len == 0 {
            BASIS
        } else {
            self.entries[self.len - 1].link
        };
        let mut stored_tag = [0u8; TAG_MAX];
        stored_tag[..tag.len()].copy_from_slice(tag);
        let mut stored_body = [0u8; BODY_MAX];
        stored_body[..body.len()].copy_from_slice(body);
        self.entries[self.len] = Entry {
            n,
            tick,
            link: link(prev, n, tick, tag, body),
            tag: stored_tag,
            tag_len: tag.len() as u8,
            body: stored_body,
            body_len: body.len() as u8,
        };
        self.len += 1;
        Ok(n)
    }

    pub fn note_lost(&mut self) {
        self.lost = self.lost.saturating_add(1);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn free(&self) -> usize {
        N - self.len
    }

    pub fn is_full(&self) -> bool {
        self.len == N
    }

    pub fn lost(&self) -> u32 {
        self.lost
    }

    pub fn head(&self) -> u64 {
        if self.len == 0 {
            BASIS
        } else {
            self.entries[self.len - 1].link
        }
    }

    pub fn admits_agent(&self, reserve: usize) -> bool {
        self.free() > reserve
    }

    pub fn get(&self, n: u32) -> Option<Entry> {
        if n == 0 || n as usize > self.len {
            None
        } else {
            Some(self.entries[n as usize - 1])
        }
    }

    pub fn verify(&self) -> Verify {
        self.verify_upto(self.len as u32)
    }

    pub fn verify_upto(&self, n: u32) -> Verify {
        if n > self.len as u32 {
            return Verify::Broken {
                at: self.len as u32 + 1,
            };
        }
        let mut prev = BASIS;
        for index in 0..n as usize {
            let entry = &self.entries[index];
            let expect = link(prev, entry.n, entry.tick, entry.tag(), entry.body());
            if entry.n != index as u32 + 1 || expect != entry.link {
                return Verify::Broken {
                    at: index as u32 + 1,
                };
            }
            prev = entry.link;
        }
        Verify::Ok
    }

    /// Flips bit 0 of body byte 0 of entry `n`. Compiled only for the proof.
    #[cfg(feature = "recorder-core")]
    pub fn corrupt_for_test(&mut self, n: u32) {
        if n >= 1 && (n as usize) <= self.len {
            self.entries[n as usize - 1].body[0] ^= 1;
        }
    }
}

fn valid_text(bytes: &[u8], min: usize, max: usize) -> bool {
    bytes.len() >= min
        && bytes.len() <= max
        && bytes.iter().all(|byte| (0x20..=0x7e).contains(byte))
}

/// A fixed body buffer. `overflow` is set when a write did not fit.
pub struct Body {
    bytes: [u8; BODY_MAX],
    len: u8,
    overflowed: bool,
}

impl Body {
    fn new() -> Self {
        Self {
            bytes: [0; BODY_MAX],
            len: 0,
            overflowed: false,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub fn overflow(&self) -> bool {
        self.overflowed
    }

    fn push_bytes(&mut self, data: &[u8]) {
        for &byte in data {
            if self.len as usize >= BODY_MAX {
                self.overflowed = true;
                return;
            }
            self.bytes[self.len as usize] = byte;
            self.len += 1;
        }
    }

    fn push_str(&mut self, text: &str) {
        self.push_bytes(text.as_bytes());
    }

    fn push_u64(&mut self, mut value: u64) {
        let mut buf = [0u8; 20];
        let mut index = 20;
        loop {
            index -= 1;
            buf[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.push_bytes(&buf[index..]);
    }
}

/// Canonical body for one gate event.
pub fn event_body(
    agent: Option<&[u8]>,
    action: &str,
    seq: Option<u32>,
    rq: Option<u32>,
    result: &str,
    id: Option<usize>,
    reason: Option<&str>,
    text: Option<&[u8]>,
) -> Body {
    let mut body = Body::new();
    if let Some(agent) = agent {
        body.push_bytes(agent);
        body.push_str(" ");
    }
    body.push_str(action);
    body.push_str(" seq=");
    match seq {
        Some(n) => body.push_u64(u64::from(n)),
        None => body.push_str("-"),
    }
    if let Some(rq) = rq {
        body.push_str(" rq=");
        body.push_u64(u64::from(rq));
    }
    body.push_str(" ");
    body.push_str(result);
    if let Some(id) = id {
        body.push_str(" id=");
        body.push_u64(id as u64);
    }
    if let Some(reason) = reason {
        body.push_str(" reason=");
        body.push_str(reason);
    }
    if let Some(text) = text {
        body.push_str(" text=");
        body.push_bytes(text);
    }
    body
}
