//! Smoke-test bodies for the allocator features. Safe code only.
//!
//! Each function checks its claims with `assert!`, so a failed check reaches
//! the panic handler and exits 35 instead of the case's own status. On success
//! it prints one line and exits with the case's status. Spec: `docs/MEMORY.md`.

#[cfg(any(feature = "heap", feature = "heap-oom-panic"))]
use alloc::vec::Vec;
#[cfg(any(feature = "heap", feature = "heap-oom"))]
use core::alloc::Layout;
#[cfg(any(
    feature = "heap",
    feature = "heap-oom",
    feature = "frames",
    feature = "frames-oom"
))]
use core::fmt::Write;

#[cfg(any(feature = "heap", feature = "heap-oom"))]
use crate::heap;
#[cfg(any(
    feature = "heap",
    feature = "heap-oom",
    feature = "frames",
    feature = "frames-oom"
))]
use crate::{qemu_exit, serial};
#[cfg(any(feature = "frames", feature = "frames-oom"))]
use crate::{frames, memmap};

#[cfg(feature = "heap")]
fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).expect("valid layout")
}

#[cfg(feature = "heap")]
pub fn run_heap() -> ! {
    use alloc::boxed::Box;
    use alloc::format;

    let boxed = Box::new(0x1122_3344_5566_7788u64);
    assert_eq!(*boxed, 0x1122_3344_5566_7788u64);

    let mut numbers: Vec<u32> = Vec::new();
    for i in 0..1000u32 {
        numbers.push(i);
    }
    let sum: u64 = numbers.iter().map(|&n| u64::from(n)).sum();
    assert_eq!(sum, 499_500);

    let text = format!("r3ath3-{}-{}", 7, "heap");
    assert_eq!(text, "r3ath3-7-heap");

    let base = heap::base();
    let top = base + heap::SIZE;
    let mut previous_end = base + heap::used();
    for align in [1usize, 8, 16, 64, 4096] {
        let size = 24;
        let ptr = heap::try_alloc(layout(size, align)).expect("aligned allocation");
        let start = ptr.as_ptr() as usize;
        assert_eq!(start % align, 0);
        assert!(start >= base && start + size <= top);
        assert!(start >= previous_end);
        previous_end = start + size;
    }

    let used_before = heap::used();
    assert!(heap::try_alloc(layout(8, 8192)).is_none());
    assert_eq!(heap::used(), used_before);

    let used = heap::used();
    assert!(used > 0 && used <= heap::SIZE);
    let _ = write!(
        serial::Writer,
        "R3ATH3_HEAP ok size={} used={} align_max={}\n",
        heap::SIZE,
        used,
        heap::MAX_ALIGN
    );
    qemu_exit::heap_ok();
}

#[cfg(feature = "heap-oom")]
pub fn run_heap_oom() -> ! {
    const CHUNK: usize = 64 * 1024;
    assert_eq!(heap::used(), 0);
    let chunk = Layout::from_size_align(CHUNK, 4096).expect("valid layout");
    let mut chunks = 0usize;
    while heap::try_alloc(chunk).is_some() {
        chunks += 1;
        assert!(chunks <= heap::SIZE / CHUNK);
    }
    assert_eq!(chunks, 16);
    assert_eq!(heap::used(), heap::SIZE);

    let one = Layout::from_size_align(1, 1).expect("valid layout");
    assert!(heap::try_alloc(one).is_none());
    assert_eq!(heap::used(), heap::SIZE);

    let _ = write!(
        serial::Writer,
        "R3ATH3_HEAP_OOM ok chunks={} used={}\n",
        chunks,
        heap::used()
    );
    qemu_exit::heap_oom_ok();
}

#[cfg(feature = "heap-oom-panic")]
pub fn run_heap_oom_panic() -> ! {
    let big: Vec<u8> = Vec::with_capacity(2 * 1024 * 1024);
    core::hint::black_box(&big);
    panic!("a 2 MiB allocation succeeded in a 1 MiB heap");
}

#[cfg(feature = "frames")]
pub fn run_frames() -> ! {
    use crate::frames::{FrameError, PhysFrame, FRAME_SIZE};

    let (total, free) = frames::stats();
    assert!(total > 0 && free == total);
    let kernel_end = frames::kernel_end();
    let top = memmap::usable_top();

    let frame = frames::alloc().expect("a free frame");
    let first = frame.addr();
    assert_eq!(first % FRAME_SIZE, 0);
    assert!(first >= kernel_end);
    assert!(first >= 1 << 20);
    assert!(first < top && first + FRAME_SIZE <= top);
    assert!(frames::frame_is_zero(frame));
    assert!(frames::write_pattern_and_verify(frame));
    assert_eq!(frames::stats().1, free - 1);

    assert_eq!(frames::free(frame), Ok(()));
    let again = frames::alloc().expect("a free frame");
    assert_eq!(again, frame);
    assert!(frames::frame_is_zero(again));
    assert_eq!(frames::free(again), Ok(()));
    assert_eq!(frames::free(again), Err(FrameError::NotAllocated));
    assert_eq!(frames::stats(), (total, free));

    assert_eq!(frames::free(PhysFrame(first + 1)), Err(FrameError::Unaligned));
    assert_eq!(frames::free(PhysFrame(1 << 30)), Err(FrameError::OutOfRange));
    assert_eq!(frames::free(PhysFrame(0)), Err(FrameError::NotAllocated));
    assert_eq!(
        frames::free(PhysFrame(1 << 20)),
        Err(FrameError::NotAllocated)
    );
    assert_eq!(frames::stats(), (total, free));

    let _ = write!(
        serial::Writer,
        "R3ATH3_FRAMES ok total={} free={} first={:#x} kernel_end={:#x}\n",
        total,
        free,
        first,
        kernel_end
    );
    qemu_exit::frames_ok();
}

#[cfg(feature = "frames-oom")]
pub fn run_frames_oom() -> ! {
    use crate::frames::FRAME_SIZE;

    let (_, free_at_init) = frames::stats();
    let top = memmap::usable_top();
    let mut allocated = 0usize;
    let mut max = 0u64;
    let mut last = None;
    while let Some(frame) = frames::alloc() {
        allocated += 1;
        max = max.max(frame.addr());
        last = Some(frame);
    }
    assert_eq!(allocated, free_at_init);
    assert!(max + FRAME_SIZE <= top);
    assert_eq!(frames::stats().1, 0);
    assert!(frames::alloc().is_none());

    let released = last.expect("at least one frame was allocated");
    assert_eq!(frames::free(released), Ok(()));
    assert_eq!(frames::alloc(), Some(released));

    let _ = write!(
        serial::Writer,
        "R3ATH3_FRAMES_OOM ok allocated={} max={:#x}\n",
        allocated,
        max
    );
    qemu_exit::frames_oom_ok();
}
