//! Kernel heap: a bump allocator over a static 1 MiB area.
//!
//! # Invariants
//! The area is a 4096-aligned array in `.bss`, inside the kernel image, so the
//! frame allocator never hands it out. The heap does not depend on `memmap` or
//! `frames`. The only state is one atomic offset, `NEXT`, from the area base.
//! `dealloc` does nothing: the heap only grows, and memory is reclaimed only by
//! reboot. Alignments 1 through 4096 are supported; a larger alignment fails.
//! One CPU. IRQ handlers must not allocate. The compare-exchange keeps the
//! bump correct even if that rule is broken later.

#![allow(dead_code)] // `used`, `base`, and `MAX_ALIGN` serve test builds

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Heap size in bytes.
pub const SIZE: usize = 1 << 20;
/// Largest supported alignment.
pub const MAX_ALIGN: usize = 4096;

#[repr(C, align(4096))]
struct HeapArea(UnsafeCell<[u8; SIZE]>);

// SAFETY: the area is only handed out in disjoint ranges. A range is reserved
// by a successful compare-exchange on `NEXT` and is never reserved again, so
// no two callers receive overlapping memory.
unsafe impl Sync for HeapArea {}

static AREA: HeapArea = HeapArea(UnsafeCell::new([0; SIZE]));
static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct BumpAllocator;

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

// SAFETY: `alloc` returns null or a pointer to a range inside `AREA` that no
// other call has received and that satisfies `layout`. `dealloc` is a no-op,
// which is valid because it never makes memory available again. `realloc` is
// the default (`alloc`, copy, `dealloc`).
unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match try_alloc(layout) {
            Some(ptr) => ptr.as_ptr(),
            None => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

fn base_ptr() -> *mut u8 {
    AREA.0.get().cast::<u8>()
}

/// Address of the first byte of the heap area.
pub fn base() -> usize {
    base_ptr() as usize
}

/// Bytes consumed so far, including alignment padding.
pub fn used() -> usize {
    NEXT.load(Ordering::Relaxed)
}

/// The path `GlobalAlloc::alloc` uses. Returns `None` for a zero size, an
/// alignment above `MAX_ALIGN`, or when the request does not fit. On `None`,
/// `NEXT` is unchanged.
pub fn try_alloc(layout: Layout) -> Option<NonNull<u8>> {
    let size = layout.size();
    let align = layout.align();
    if size == 0 || align > MAX_ALIGN {
        return None;
    }
    let base = base_ptr();
    let base_addr = base as usize;
    let limit = base_addr.checked_add(SIZE)?;
    let mut next = NEXT.load(Ordering::Relaxed);
    loop {
        let start = base_addr
            .checked_add(next)?
            .checked_add(align - 1)?
            & !(align - 1);
        let end = start.checked_add(size)?;
        if end > limit {
            return None;
        }
        match NEXT.compare_exchange(next, end - base_addr, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return NonNull::new(base.wrapping_add(start - base_addr)),
            Err(current) => next = current,
        }
    }
}
