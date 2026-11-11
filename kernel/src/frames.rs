//! Physical frame allocator: 4 KiB frames from usable RAM below 1 GiB.
//!
//! # Invariants
//! Coverage is `[0, 1 GiB)`, which is 262,144 frames. Two static bitmaps hold
//! the state. `used` has a 1 for every frame that is allocated or unavailable;
//! `init` sets every bit to 1 before clearing any. `managed` has a 1 for every
//! frame the allocator may ever hand out: type-1 RAM in `[1 MiB, 1 GiB)` minus
//! the kernel image, the start-info struct, and the memory-map table. `alloc`
//! returns the lowest free frame, zeroed through the identity map. `free`
//! accepts only a managed frame that is currently allocated.
//!
//! Main context only. IRQ handlers must not call in. A busy flag turns
//! reentry into a panic (`frame allocator reentered`).

#![allow(dead_code)] // `stats`, `kernel_end`, and the frame checks serve test builds

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::mem::MemError;
use crate::memmap;

pub const FRAME_SIZE: u64 = 4096;
const COVERAGE: u64 = 1 << 30;
const LOW_LIMIT: u64 = 1 << 20;
const FRAME_COUNT: usize = (COVERAGE / FRAME_SIZE) as usize;
const WORDS: usize = FRAME_COUNT / 64;

/// A physical frame address. `alloc` only returns 4 KiB-aligned addresses
/// below 1 GiB. The field is public so callers can build a value to pass to
/// `free`, which validates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysFrame(pub u64);

impl PhysFrame {
    pub fn addr(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Not a multiple of 4 KiB. Checked first.
    Unaligned,
    /// At or above 1 GiB.
    OutOfRange,
    /// Double free, or a frame that was never handed out by the allocator.
    NotAllocated,
}

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

struct Map {
    used: [u64; WORDS],
    managed: [u64; WORDS],
    /// Every word below this index is full.
    hint: usize,
    total: usize,
    free: usize,
}

struct State(UnsafeCell<Map>);

// SAFETY: every access goes through `with_map`, which holds the `BUSY` flag
// for its whole duration, so there is never more than one live reference.
unsafe impl Sync for State {}

static STATE: State = State(UnsafeCell::new(Map {
    used: [0; WORDS],
    managed: [0; WORDS],
    hint: 0,
    total: 0,
    free: 0,
}));
static BUSY: AtomicBool = AtomicBool::new(false);

struct Guard;

impl Guard {
    fn enter() -> Self {
        if BUSY.swap(true, Ordering::Acquire) {
            panic!("frame allocator reentered");
        }
        Guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

fn with_map<R>(f: impl FnOnce(&mut Map) -> R) -> R {
    let _guard = Guard::enter();
    // SAFETY: `_guard` owns `BUSY`, and `Guard::enter` panics rather than
    // returning while another guard exists, so this is the only reference.
    f(unsafe { &mut *STATE.0.get() })
}

fn split(index: usize) -> (usize, u64) {
    (index / 64, 1u64 << (index % 64))
}

impl Map {
    fn make_free(&mut self, index: usize) {
        let (word, mask) = split(index);
        self.used[word] &= !mask;
        self.managed[word] |= mask;
    }

    fn reserve(&mut self, start: u64, end: u64) {
        let first = (start / FRAME_SIZE) as usize;
        let last = (end.div_ceil(FRAME_SIZE) as usize).min(FRAME_COUNT);
        for index in first..last {
            let (word, mask) = split(index);
            self.used[word] |= mask;
            self.managed[word] &= !mask;
        }
    }

    fn take(&mut self) -> Option<u64> {
        let mut word = self.hint;
        while word < WORDS {
            let free_bits = !self.used[word];
            if free_bits != 0 {
                let bit = free_bits.trailing_zeros() as usize;
                self.used[word] |= 1u64 << bit;
                self.hint = word;
                self.free -= 1;
                return Some(((word * 64 + bit) as u64) * FRAME_SIZE);
            }
            word += 1;
        }
        self.hint = WORDS;
        None
    }

    fn give_back(&mut self, index: usize) -> Result<(), FrameError> {
        let (word, mask) = split(index);
        if self.managed[word] & mask == 0 || self.used[word] & mask == 0 {
            return Err(FrameError::NotAllocated);
        }
        self.used[word] &= !mask;
        self.free += 1;
        self.hint = self.hint.min(word);
        Ok(())
    }

    fn is_allocated(&self, index: usize) -> bool {
        let (word, mask) = split(index);
        self.managed[word] & mask != 0 && self.used[word] & mask != 0
    }
}

fn kernel_range() -> (u64, u64) {
    // Both symbols are defined by `linker.ld`. Only their addresses are taken;
    // the bytes are never read.
    (
        ptr::addr_of!(__kernel_start) as u64,
        ptr::addr_of!(__kernel_end) as u64,
    )
}

/// First byte after the kernel image (code, data, `.bss`, page tables, stacks,
/// the heap, and these bitmaps). 4 KiB-aligned.
pub fn kernel_end() -> u64 {
    kernel_range().1
}

fn zero_frame(addr: u64) {
    // SAFETY: `addr` is a frame this call just took from the bitmap, so it is
    // managed RAM below 1 GiB (identity mapped), outside the kernel image and
    // start info, and owned by nobody else. 512 qwords is exactly one frame.
    // `rep stosq` needs DF clear, which the ABI and `cld` in the entry stubs
    // guarantee. RAX, RCX, and RDI are declared.
    unsafe {
        asm!(
            "rep stosq",
            inout("rcx") 512usize => _,
            inout("rdi") addr => _,
            in("rax") 0u64,
            options(nostack, preserves_flags)
        );
    }
}

/// Build the bitmaps from the memory map. Needs a successful `memmap::init`.
pub fn init() -> Result<(), MemError> {
    let (kernel_start, kernel_end) = kernel_range();
    let (info_start, info_end) = memmap::start_info_range();
    let (table_start, table_end) = memmap::table_range();
    with_map(|map| {
        map.used.fill(u64::MAX);
        map.managed.fill(0);
        map.hint = 0;
        for region in memmap::regions() {
            if region.kind != memmap::KIND_RAM {
                continue;
            }
            let start = region.addr.max(LOW_LIMIT);
            let end = region.addr.saturating_add(region.size).min(COVERAGE);
            if start >= end {
                continue;
            }
            let first = start.div_ceil(FRAME_SIZE);
            let last = end / FRAME_SIZE;
            for index in first..last {
                map.make_free(index as usize);
            }
        }
        map.reserve(kernel_start, kernel_end);
        map.reserve(info_start, info_end);
        map.reserve(table_start, table_end);
        let total: u32 = map.managed.iter().map(|word| word.count_ones()).sum();
        map.total = total as usize;
        map.free = map.total;
        if map.total == 0 {
            return Err(MemError::NoUsable);
        }
        Ok(())
    })
}

/// The lowest free frame, zeroed. `None` when no frame is free.
pub fn alloc() -> Option<PhysFrame> {
    with_map(|map| {
        let addr = map.take()?;
        zero_frame(addr);
        Some(PhysFrame(addr))
    })
}

/// Return a frame. Never panics on bad input.
pub fn free(frame: PhysFrame) -> Result<(), FrameError> {
    let addr = frame.0;
    if addr % FRAME_SIZE != 0 {
        return Err(FrameError::Unaligned);
    }
    if addr >= COVERAGE {
        return Err(FrameError::OutOfRange);
    }
    with_map(|map| map.give_back((addr / FRAME_SIZE) as usize))
}

/// `(managed frames, free frames)`. A managed frame is one the allocator can
/// hand out; the kernel image and start-info pages are not counted.
pub fn stats() -> (usize, usize) {
    with_map(|map| (map.total, map.free))
}

/// Test helper: is every byte of an allocated frame zero? `false` for a frame
/// that is not currently allocated. Compiled only for the `frames` test.
#[cfg(feature = "frames")]
pub fn frame_is_zero(frame: PhysFrame) -> bool {
    if !owned(frame) {
        return false;
    }
    let base = frame.0 as *const u8;
    for offset in 0..FRAME_SIZE as usize {
        // SAFETY: `owned` proves the frame is allocated and managed RAM;
        // exclusivity holds because the only caller is the `frames` test,
        // which allocated it and has not freed it.
        if unsafe { base.add(offset).read_volatile() } != 0 {
            return false;
        }
    }
    true
}

/// Test helper: write a byte pattern over an allocated frame and read it back.
/// `false` on a mismatch or for a frame that is not currently allocated.
/// Compiled only for the `frames` test.
#[cfg(feature = "frames")]
pub fn write_pattern_and_verify(frame: PhysFrame) -> bool {
    if !owned(frame) {
        return false;
    }
    let base = frame.0 as *mut u8;
    for offset in 0..FRAME_SIZE as usize {
        // SAFETY: `owned` proves the frame is allocated and managed RAM;
        // exclusivity holds because the only caller is the `frames` test,
        // which allocated it and has not freed it.
        unsafe { base.add(offset).write_volatile((offset as u8) ^ 0xA5) };
    }
    for offset in 0..FRAME_SIZE as usize {
        // SAFETY: `owned` proves the frame is allocated and managed RAM;
        // exclusivity holds because the only caller is the `frames` test,
        // which allocated it and has not freed it.
        if unsafe { base.add(offset).read_volatile() } != (offset as u8) ^ 0xA5 {
            return false;
        }
    }
    true
}

#[cfg(feature = "frames")]
fn owned(frame: PhysFrame) -> bool {
    frame.0 % FRAME_SIZE == 0
        && frame.0 < COVERAGE
        && with_map(|map| map.is_allocated((frame.0 / FRAME_SIZE) as usize))
}
