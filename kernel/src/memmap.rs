//! PVH start-info parser and the saved physical memory map.
//!
//! # Invariants
//! The boot stub stores `%ebx` in `pvh_start_info`. `init` runs once, on the
//! boot processor, with the 1 GiB identity map on. It validates the
//! `hvm_start_info` struct in the order listed in `docs/MEMORY.md` and copies
//! up to 32 memory-map entries into a static table. Every raw read is at an
//! address already checked to lie below 1 GiB. No reference into the PVH
//! memory is kept. After `init` succeeds the table is read-only.
//!
//! Layout (Xen `hvm/start_info.h`, little-endian): `magic u32 @0`,
//! `version u32 @4`, `memmap_paddr u64 @40`, `memmap_entries u32 @48`; the
//! struct is 56 bytes. Each memory-map entry is 24 bytes: `addr u64`,
//! `size u64`, `type u32`, `reserved u32`. Type 1 is usable RAM.

#![allow(dead_code)] // test-only helpers and accessors used by some builds

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::mem::MemError;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const START_INFO_SIZE: u64 = 56;
const START_INFO_MAGIC: u32 = 0x336E_C578;
const ENTRY_SIZE: u64 = 24;

pub const MAX_REGIONS: usize = 32;
pub const KIND_RAM: u32 = 1;

#[derive(Clone, Copy)]
pub struct Region {
    pub addr: u64,
    pub size: u64,
    pub kind: u32,
}

const EMPTY: Region = Region {
    addr: 0,
    size: 0,
    kind: 0,
};

unsafe extern "C" {
    #[allow(non_upper_case_globals)] // symbol name is fixed by `boot.rs`
    static mut pvh_start_info: u32;
}

struct Table(UnsafeCell<[Region; MAX_REGIONS]>);

// SAFETY: `init` is the only writer. It runs once on the boot CPU, before
// `COUNT` is published with Release, and nothing writes the table afterwards.
unsafe impl Sync for Table {}

static TABLE: Table = Table(UnsafeCell::new([EMPTY; MAX_REGIONS]));
static COUNT: AtomicUsize = AtomicUsize::new(0);
static INFO_ADDR: AtomicU64 = AtomicU64::new(0);
static MAP_ADDR: AtomicU64 = AtomicU64::new(0);
static MAP_BYTES: AtomicU64 = AtomicU64::new(0);

fn saved_start_info() -> u32 {
    // SAFETY: `pvh_start_info` is a 4-byte, 4-aligned symbol defined in
    // `boot.rs`, written once by `_start` before any Rust code runs. Only its
    // address is taken, then it is read with a volatile load.
    unsafe { ptr::addr_of!(pvh_start_info).read_volatile() }
}

/// Test hook for the `mem-badinfo` build: forget the saved pointer so that
/// `init` must reject the start info.
#[cfg(feature = "mem-badinfo")]
pub fn corrupt_start_info_for_test() {
    // SAFETY: same symbol as `saved_start_info`. The kernel is single-CPU and
    // no other code reads it between `_start` and `init`.
    unsafe { ptr::addr_of_mut!(pvh_start_info).write_volatile(0) }
}

/// Volatile read of a start-info field. `addr` is 4-byte aligned because the
/// struct is 8-byte aligned and every field offset is a multiple of 4.
fn info_u32(addr: u64) -> u32 {
    // SAFETY: the caller passes `info + offset` with `info` validated as
    // 8-aligned and `info + 56 <= 1 GiB`, so the 4 bytes are inside the
    // identity map and aligned.
    unsafe { ptr::read_volatile(addr as *const u32) }
}

fn info_u64(addr: u64) -> u64 {
    // SAFETY: as `info_u32`; the offset is a multiple of 8, so the 8 bytes are
    // aligned and inside the validated struct (the only u64 field read is at
    // offset 40).
    unsafe { ptr::read_volatile(addr as *const u64) }
}

fn entry_u32(addr: u64) -> u32 {
    // SAFETY: the caller passes an address inside the memory-map table, whose
    // whole extent was checked to be below 1 GiB (identity mapped). The table
    // may be unaligned, so the read is unaligned.
    unsafe { ptr::read_unaligned(addr as *const u32) }
}

fn entry_u64(addr: u64) -> u64 {
    // SAFETY: as `entry_u32`.
    unsafe { ptr::read_unaligned(addr as *const u64) }
}

pub fn init() -> Result<(), MemError> {
    if COUNT.load(Ordering::Acquire) != 0 {
        return Ok(());
    }
    let info = u64::from(saved_start_info());
    if info == 0
        || info % 8 != 0
        || info.checked_add(START_INFO_SIZE).map_or(true, |end| end > GIB)
    {
        return Err(MemError::StartInfo);
    }
    if info_u32(info) != START_INFO_MAGIC {
        return Err(MemError::StartInfo);
    }
    if info_u32(info + 4) < 1 {
        return Err(MemError::MemMap);
    }
    let map = info_u64(info + 40);
    let entries = u64::from(info_u32(info + 48));
    if entries < 1 || entries > MAX_REGIONS as u64 {
        return Err(MemError::MemMap);
    }
    let bytes = entries * ENTRY_SIZE;
    if map.checked_add(bytes).map_or(true, |end| end > GIB) {
        return Err(MemError::MemMap);
    }

    let mut staged = [EMPTY; MAX_REGIONS];
    let mut has_ram = false;
    for i in 0..entries {
        let at = map + i * ENTRY_SIZE;
        let region = Region {
            addr: entry_u64(at),
            size: entry_u64(at + 8),
            kind: entry_u32(at + 16),
        };
        if region.kind == KIND_RAM && overlaps_low_gib(&region) {
            has_ram = true;
        }
        staged[i as usize] = region;
    }
    if !has_ram {
        return Err(MemError::NoUsable);
    }

    let table = TABLE.0.get().cast::<Region>();
    for (i, region) in staged.iter().enumerate().take(entries as usize) {
        // SAFETY: `i < entries <= MAX_REGIONS`, so the slot is inside the
        // static table, and nothing reads the table until COUNT is published.
        unsafe { table.add(i).write(*region) };
    }
    INFO_ADDR.store(info, Ordering::Relaxed);
    MAP_ADDR.store(map, Ordering::Relaxed);
    MAP_BYTES.store(bytes, Ordering::Relaxed);
    COUNT.store(entries as usize, Ordering::Release);
    Ok(())
}

fn overlaps_low_gib(region: &Region) -> bool {
    region.size != 0 && region.addr < GIB && region.addr.saturating_add(region.size) > MIB
}

/// The copied memory-map entries. Empty before a successful `init`.
pub fn regions() -> &'static [Region] {
    let count = COUNT.load(Ordering::Acquire);
    // SAFETY: `count` entries were fully written before COUNT was published
    // with Release, and the table is never written again. With count 0 the
    // slice is empty and the pointer is still valid and aligned.
    unsafe { core::slice::from_raw_parts(TABLE.0.get().cast::<Region>(), count) }
}

/// Physical `[start, end)` of the `hvm_start_info` struct.
pub fn start_info_range() -> (u64, u64) {
    let start = INFO_ADDR.load(Ordering::Relaxed);
    (start, start + START_INFO_SIZE)
}

/// Physical `[start, end)` of the memory-map table.
pub fn table_range() -> (u64, u64) {
    let start = MAP_ADDR.load(Ordering::Relaxed);
    (start, start + MAP_BYTES.load(Ordering::Relaxed))
}

/// One past the highest byte of type-1 RAM that lies in `[1 MiB, 1 GiB)`.
pub fn usable_top() -> u64 {
    let mut top = 0;
    for region in regions() {
        if region.kind != KIND_RAM {
            continue;
        }
        let start = region.addr.max(MIB);
        let end = region.addr.saturating_add(region.size).min(GIB);
        if start < end && end > top {
            top = end;
        }
    }
    top
}
