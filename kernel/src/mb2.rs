//! Multiboot2 information block.
//!
//! # Invariants
//! `_start_mb2` stores the magic and the info pointer. `parse` runs once, on
//! the boot processor, with the 1 GiB identity map on. The block must be
//! 8-byte aligned, at most 64 KiB, and entirely below 1 GiB. Tags are 8-byte
//! aligned. Every raw read is inside that checked block. No reference into
//! the firmware memory is kept. Type 1 memory-map entries are RAM, the same
//! numbering `memmap` already uses. The framebuffer is recorded only. A
//! linear console is installed later, and only for RGB at 32 bpp.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::mem::MemError;

const GIB: u64 = 1 << 30;
const INFO_MAX: u64 = 64 * 1024;
const MAGIC: u32 = 0x36D7_6289;
pub const MAX_REGIONS: usize = 32;

const TAG_END: u32 = 0;
const TAG_MMAP: u32 = 6;
const TAG_FB: u32 = 8;
const TAG_EFI32: u32 = 11;
const TAG_EFI64: u32 = 12;
const TAG_EFI_MMAP: u32 = 17;
const TAG_EFI_IH32: u32 = 19;
const TAG_EFI_IH64: u32 = 20;

const FB_RGB: u8 = 1;
#[allow(dead_code)] // text framebuffers are recorded; `vga` draws them
const FB_TEXT: u8 = 2;

#[derive(Clone, Copy)]
pub struct Region {
    pub addr: u64,
    pub size: u64,
    pub kind: u32,
}

#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub kind: u8,
    pub red_pos: u8,
    pub red_size: u8,
    pub green_pos: u8,
    pub green_size: u8,
    pub blue_pos: u8,
    pub blue_size: u8,
}

pub struct Parsed {
    pub regions: [Region; MAX_REGIONS],
    pub count: usize,
    pub raw_entries: u32,
    pub type1_bytes: u64,
    pub uefi: bool,
    pub fb: Option<Framebuffer>,
    pub info_start: u64,
    pub info_end: u64,
}

struct Summary {
    uefi: bool,
    count: usize,
    raw: u32,
    type1: u64,
    fb: Option<Framebuffer>,
}

struct SummaryCell(UnsafeCell<Summary>);

// SAFETY: `parse` is the only writer. It runs once on the boot CPU, before
// `READY` is published with Release, and nothing writes the summary afterwards.
unsafe impl Sync for SummaryCell {}

static SUMMARY: SummaryCell = SummaryCell(UnsafeCell::new(Summary {
    uefi: false,
    count: 0,
    raw: 0,
    type1: 0,
    fb: None,
}));
static READY: AtomicBool = AtomicBool::new(false);

// SAFETY: `mb2_info` and `mb2_magic` are 4-byte, 4-aligned symbols in
// `boot.rs`. `_start_mb2` writes them before Rust runs. This declaration
// only names them. The reads are below.
unsafe extern "C" {
    static mut mb2_info: u32;
    static mut mb2_magic: u32;
}

fn saved_info() -> u32 {
    // SAFETY: `mb2_info` is a 4-byte, 4-aligned symbol in `boot.rs`, written
    // once by `_start_mb2` before Rust runs. Only the address is taken.
    unsafe { ptr::addr_of!(mb2_info).read_volatile() }
}

fn saved_magic() -> u32 {
    // SAFETY: same symbol rule as `saved_info`, for `mb2_magic`.
    unsafe { ptr::addr_of!(mb2_magic).read_volatile() }
}

/// The CPU entered through `_start_mb2` and `%eax` held the Multiboot2 magic.
pub fn valid() -> bool {
    saved_magic() == MAGIC && saved_info() != 0
}

fn summary() -> Option<&'static Summary> {
    if !READY.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: READY was published with Release after the summary was written,
    // and it is never written again.
    Some(unsafe { &*SUMMARY.0.get() })
}

pub fn firmware_uefi() -> bool {
    summary().map(|item| item.uefi).unwrap_or(false)
}

pub fn stored_entries() -> usize {
    summary().map(|item| item.count).unwrap_or(0)
}

pub fn raw_entries() -> u32 {
    summary().map(|item| item.raw).unwrap_or(0)
}

pub fn type1_bytes() -> u64 {
    summary().map(|item| item.type1).unwrap_or(0)
}

pub fn framebuffer() -> Option<Framebuffer> {
    summary().and_then(|item| item.fb)
}

fn read_u32(addr: u64) -> u32 {
    // SAFETY: callers pass an address inside the info block already checked
    // to lie below 1 GiB. The read is unaligned so a tag field that is only
    // 4-byte aligned is still defined.
    unsafe { ptr::read_unaligned(addr as *const u32) }
}

fn read_u64(addr: u64) -> u64 {
    // SAFETY: as `read_u32`.
    unsafe { ptr::read_unaligned(addr as *const u64) }
}

fn read_u8(addr: u64) -> u8 {
    // SAFETY: as `read_u32`. One byte needs no alignment.
    unsafe { ptr::read_volatile(addr as *const u8) }
}

/// Validate the block and copy up to 32 memory-map entries. Type-1 entries
/// are kept first when the firmware map is longer than the table, and a
/// type-1 entry that does not fit is an error.
pub fn parse() -> Result<Parsed, MemError> {
    if !valid() {
        return Err(MemError::Mb2);
    }
    let info = u64::from(saved_info());
    if info % 8 != 0 {
        return Err(MemError::Mb2);
    }
    let total = u64::from(read_u32(info));
    if total < 8 || total > INFO_MAX {
        return Err(MemError::Mb2);
    }
    let Some(end) = info.checked_add(total) else {
        return Err(MemError::Mb2);
    };
    if end > GIB {
        return Err(MemError::Mb2);
    }

    let mut raw: [Region; 128] = [Region {
        addr: 0,
        size: 0,
        kind: 0,
    }; 128];
    let mut raw_count: usize = 0;
    let mut truncated = false;
    let mut uefi = false;
    let mut fb = None;
    let mut off = 8u64;
    let mut saw_end = false;
    while off + 8 <= total {
        let kind = read_u32(info + off);
        let size = u64::from(read_u32(info + off + 4));
        if size < 8 || off + size > total {
            return Err(MemError::Mb2);
        }
        if kind == TAG_END {
            saw_end = true;
            break;
        }
        match kind {
            TAG_MMAP => read_mmap(info, off, size, &mut raw, &mut raw_count, &mut truncated)?,
            TAG_FB => fb = Some(read_fb(info, off, size)?),
            TAG_EFI32 | TAG_EFI64 | TAG_EFI_MMAP | TAG_EFI_IH32 | TAG_EFI_IH64 => uefi = true,
            _ => {}
        }
        let next = (off + size + 7) & !7;
        if next <= off || next > total {
            return Err(MemError::Mb2);
        }
        off = next;
    }
    if !saw_end {
        return Err(MemError::Mb2);
    }
    if truncated {
        return Err(MemError::Mb2);
    }

    let mut type1_bytes = 0u64;
    let mut staged = [Region {
        addr: 0,
        size: 0,
        kind: 0,
    }; MAX_REGIONS];
    let mut count = 0usize;
    for region in raw.iter().take(raw_count).filter(|region| region.kind == 1) {
        type1_bytes = type1_bytes.saturating_add(region.size);
        if count >= MAX_REGIONS {
            return Err(MemError::Mb2);
        }
        staged[count] = *region;
        count += 1;
    }
    for region in raw.iter().take(raw_count).filter(|region| region.kind != 1) {
        if count >= MAX_REGIONS {
            break;
        }
        staged[count] = *region;
        count += 1;
    }
    if count == 0 {
        return Err(MemError::Mb2);
    }

    let summary = Summary {
        uefi,
        count,
        raw: raw_count as u32,
        type1: type1_bytes,
        fb,
    };
    // SAFETY: this is the only write, on the boot CPU, before READY is set.
    // Callers of the accessors wait for that Release store.
    unsafe {
        SUMMARY.0.get().write(summary);
    }
    READY.store(true, Ordering::Release);

    Ok(Parsed {
        regions: staged,
        count,
        raw_entries: raw_count as u32,
        type1_bytes,
        uefi,
        fb,
        info_start: info,
        info_end: end,
    })
}

fn read_mmap(
    info: u64,
    off: u64,
    size: u64,
    raw: &mut [Region; 128],
    raw_count: &mut usize,
    truncated: &mut bool,
) -> Result<(), MemError> {
    if size < 16 {
        return Err(MemError::Mb2);
    }
    let entry_size = u64::from(read_u32(info + off + 8));
    if entry_size < 24 {
        return Err(MemError::Mb2);
    }
    let mut pos = off + 16;
    let tag_end = off + size;
    while pos + entry_size <= tag_end {
        let region = Region {
            addr: read_u64(info + pos),
            size: read_u64(info + pos + 8),
            kind: read_u32(info + pos + 16),
        };
        if *raw_count == raw.len() {
            if region.kind == 1 {
                *truncated = true;
            }
        } else {
            raw[*raw_count] = region;
            *raw_count += 1;
        }
        pos += entry_size;
    }
    Ok(())
}

fn read_fb(info: u64, off: u64, size: u64) -> Result<Framebuffer, MemError> {
    if size < 32 {
        return Err(MemError::Mb2);
    }
    let kind = read_u8(info + off + 29);
    let mut fb = Framebuffer {
        addr: read_u64(info + off + 8),
        pitch: read_u32(info + off + 16),
        width: read_u32(info + off + 20),
        height: read_u32(info + off + 24),
        bpp: read_u8(info + off + 28),
        kind,
        red_pos: 0,
        red_size: 0,
        green_pos: 0,
        green_size: 0,
        blue_pos: 0,
        blue_size: 0,
    };
    if kind == FB_RGB {
        if size < 38 {
            return Err(MemError::Mb2);
        }
        fb.red_pos = read_u8(info + off + 32);
        fb.red_size = read_u8(info + off + 33);
        fb.green_pos = read_u8(info + off + 34);
        fb.green_size = read_u8(info + off + 35);
        fb.blue_pos = read_u8(info + off + 36);
        fb.blue_size = read_u8(info + off + 37);
    }
    Ok(fb)
}
