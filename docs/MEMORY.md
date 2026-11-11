# Memory allocator (milestone 5)

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the kernel heap and the physical frame allocator. If the code and this document disagree, the change is not done.

## 1. Facts this design rests on

- The boot stub (`kernel/src/boot.rs`) identity-maps 0–1 GiB with 2 MiB pages and calls `kernel_main` on a 16 KiB stack. The kernel links at 1 MiB (`kernel/linker.ld`).
- The smoke harness boots QEMU with `-m 64M`. The identity map is 1 GiB, but only about 64 MiB of it is RAM. The frame allocator never hands out a frame just because it is mapped. It uses the real memory map.
- QEMU enters through PVH. At `_start`, `%ebx` holds the physical address of the `hvm_start_info` struct, which carries the memory map.
- There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0 targeting `x86_64-unknown-none`, which ships `core` and `alloc`, so `extern crate alloc` needs nothing new.

## 2. Scope

In scope:

1. A kernel heap: a bump allocator registered as `#[global_allocator]`, so `alloc::{Box, Vec, String}` work.
2. A physical frame allocator: 4 KiB frames from usable RAM inside the 1 GiB identity map, with allocate and free.
3. Saving and parsing the PVH memory map.
4. Smoke cases with their own statuses.

Not in this milestone: new page tables or remapping, guard pages, a heap that grows from frames, a free-list or slab allocator, allocating from IRQ handlers, multi-CPU support, and any change to the scheduler, shell, or file roadmap items.

## 3. Boot change: keep the PVH start-info pointer

In `boot.rs`, `_start` stores `%ebx` into the 4-byte `.bss.boot` symbol `pvh_start_info` as its first action after `cli`, before anything can clobber `%ebx`. Nothing else in the stub changes.

## 4. Memory map (`kernel/src/memmap.rs`)

`memmap::init()` reads `pvh_start_info` and validates it in this order. The first failure is fatal (section 8).

| Check | Failure reason word |
| --- | --- |
| Pointer is nonzero, 8-byte aligned, and the whole 56-byte struct is below 1 GiB | `startinfo` |
| `magic == 0x336EC578` | `startinfo` |
| `version >= 1` (memmap fields exist) | `memmap` |
| `memmap_entries` is 1–32 and the whole table is below 1 GiB | `memmap` |
| At least one type-1 (RAM) entry overlaps [1 MiB, 1 GiB) | `nousable` |

Layout, from Xen `hvm/start_info.h`, all little-endian:

- `hvm_start_info`: `magic u32 @0`, `version u32 @4`, `flags u32 @8`, `nr_modules u32 @12`, `modlist_paddr u64 @16`, `cmdline_paddr u64 @24`, `rsdp_paddr u64 @32`, `memmap_paddr u64 @40`, `memmap_entries u32 @48`, `reserved u32 @52`. The struct is 56 bytes.
- Each memmap entry is 24 bytes: `addr u64`, `size u64`, `type u32`, `reserved u32`. Only type 1 is usable RAM.

The entries are copied into a static `[Region; 32]`. Raw memory is read only with `read_volatile` (start-info fields, which are aligned) or `read_unaligned` (table entries), on addresses already checked to be below 1 GiB. No reference into the PVH memory is kept. A second call to `init` after a success does nothing.

There is no fallback. If QEMU does not supply a version-1 memory map, every build exits 201 (section 8); the allocator never assumes a RAM size.

Accessors: `regions()`, `start_info_range()`, `table_range()`, and `usable_top()`, which is one past the highest type-1 byte inside [1 MiB, 1 GiB).

## 5. Kernel heap (`kernel/src/heap.rs`)

Where it lives: a static 1 MiB array in `.bss`, `#[repr(C, align(4096))] struct HeapArea(UnsafeCell<[u8; SIZE]>)`, with `SIZE = 1 << 20` (1,048,576 bytes). It is inside the kernel image, below `__kernel_end`, so the frame allocator never hands it out. It does not depend on the frame allocator or the memory map, and it works even if memmap init fails. The size is the one constant `heap::SIZE`.

Why 1 MiB: it is larger than anything the next milestones (shell, a small scheduler, RAM-disk metadata) need, and small enough that the image stays small inside 64 MiB of QEMU RAM.

Algorithm:

- State is one `AtomicUsize NEXT`, an offset from the heap base, starting at 0.
- `alloc(layout)` loops on `NEXT.load`, computes `start = align_up(base + next, layout.align())` with checked arithmetic, then `end = start.checked_add(layout.size())`. If any step overflows or `end > base + SIZE`, it returns null and leaves `NEXT` unchanged. Otherwise it does `compare_exchange(next, end - base)` and retries on contention.
- `dealloc` is a no-op. Freed memory is never reused. **The heap only grows; memory is reclaimed only by reboot.**
- `realloc` is the default (`alloc`, copy, `dealloc`).
- Helpers for tests: `heap::used()`, `heap::base()`, `heap::SIZE`, `heap::MAX_ALIGN`, and `heap::try_alloc(Layout) -> Option<NonNull<u8>>`, which is the code path `alloc` uses.

Alignment:

- Supported alignment is 1–4096. A request with `align > 4096` returns null (`None` from `try_alloc`). This limit is part of the spec.
- The heap base is 4096-aligned, so a 4096-aligned request gets a page-aligned address.
- Padding skipped for alignment counts as used and is never reclaimed.
- A zero-size request through `try_alloc` returns `None`. The global allocator path never sees size 0, because `alloc` collections handle zero-size types themselves.

Concurrency: one CPU. **IRQ handlers must not allocate.** `irq.rs` stays allocation-free. The compare-exchange keeps the bump correct even if that rule is broken later.

## 6. Frame allocator (`kernel/src/frames.rs`)

- Frame size is 4 KiB. Coverage is [0, 1 GiB), which is 262,144 frames.
- Storage is two static bitmaps of 262,144 bits each (32 KiB each, `[u64; 4096]` in `.bss`):
  - `used`: 1 means allocated or unavailable. `init` sets every bit to 1 first and does not rely on `.bss` being zero.
  - `managed`: 1 means the allocator may hand this frame out. It exists so that `free` can tell a frame that was never usable (kernel image, start info, non-RAM) from a frame that is allocated. With one bitmap the two look the same, and `free` could release a kernel frame.
- For each type-1 region, clip it to [1 MiB, 1 GiB), round the start up and the end down to 4 KiB, clear the `used` bits, and set the `managed` bits.
- Then set `used` and clear `managed` for:
  - [`__kernel_start`, `__kernel_end`). This covers code, data, `.bss`, page tables, both stacks, the heap, and the bitmaps themselves. `linker.ld` defines `__kernel_start` at 1 MiB and `__kernel_end` after `.bss`, 4 KiB-aligned.
  - The frames holding `hvm_start_info` and the memmap table.
  - Everything below 1 MiB is never handed out, because the clip starts at 1 MiB.
- If no managed frame is left after that, `init` fails with `nousable`.

API, main context only:

- `frames::alloc() -> Option<PhysFrame>`: first-fit, lowest free frame. It zeroes the 4 KiB through the identity map before returning. A hint (every bitmap word below it is full) keeps the scan short; it does not change which frame is returned.
- `frames::free(PhysFrame) -> Result<(), FrameError>`: checks in this order. `Unaligned` if not a multiple of 4 KiB, `OutOfRange` if at or above 1 GiB, `NotAllocated` if the frame is not managed (never usable) or not currently allocated (double free). It never panics.
- `frames::stats() -> (total, free)`. `total` counts managed frames: the frames the allocator can hand out, which excludes the kernel image and start-info pages. `total - free` is the number allocated.
- `PhysFrame(pub u64)` is a newtype. Values returned by `alloc` are 4 KiB-aligned and below 1 GiB. The field is public so callers can pass a made-up value to `free`, which validates it.
- Test helpers: `frames::kernel_end()`, `frames::frame_is_zero(PhysFrame)`, and `frames::write_pattern_and_verify(PhysFrame)`. The last two return `false` for a frame that is not currently allocated, so they cannot touch memory the allocator does not own.
- Reentry guard: an `AtomicBool` busy flag. Reentry, such as a future IRQ handler calling in, panics with `frame allocator reentered`.

## 7. Plugging into `main.rs`

- `extern crate alloc;` and `mod heap; mod memmap; mod frames; mod mem;`.
- `mem.rs` is safe code. `mem::init()` calls `memmap::init()` and then `frames::init()`, and returns `Result<(), MemError>`. `MemError` is `StartInfo`, `MemMap`, or `NoUsable`, printed as `startinfo`, `memmap`, or `nousable`.
- In `kernel_main`, the order is `serial::init()`, the stage line, `exceptions::init()`, then `mem::init()`. A fault during memory init therefore gets a fault line. On `Err`, the kernel prints the error line and exits 201 (section 8).
- Memory init runs in every build, so every existing case also exercises it. On success it prints nothing, so every existing expected log is unchanged.
- The `#[global_allocator]` static lives in `heap.rs`.
- The new test features are in the compile-time "at most one test feature" assert and in the `cfg(not(any(...)))` lists for `boot_ok`.
- Test bodies are in `memtest.rs`, safe code that uses only the public APIs above.

## 8. Running out of memory

| Situation | Behavior | Proof |
| --- | --- | --- |
| Heap full, through `try_alloc` | Returns `None`; `NEXT` is unchanged | `heap-oom` (195) |
| Heap full, through `Box`, `Vec`, or `String` | `alloc` calls `handle_alloc_error`. On stable `no_std` this panics with `memory allocation of N bytes failed`, which reaches the panic handler: an `R3ATH3_PANIC` line, then status 35 | `heap-oom-panic` (35) |
| No free frames | `frames::alloc()` returns `None`, with no panic. Freeing a frame makes the next alloc succeed | `frames-oom` (199) |
| Bad or missing PVH info or memmap at boot | Prints `R3ATH3_MEM_ERROR reason=<startinfo\|memmap\|nousable>`, then exits status 201 | `mem-badinfo` (201) |

`heap-oom-panic` reuses 35 on purpose, because the proof is that the real panic path fires. A dedicated status for this path needs a custom alloc error handler, which is unstable on 1.83, so it is not part of this milestone.

## 9. Exit statuses (`kernel/src/qemu_exit.rs`)

QEMU turns an `out` of `V` to port `0xF4` into status `(V << 1) | 1`. `0x60` gives 193 (heap ok), `0x61` gives 195 (heap exhaustion proof), `0x62` gives 197 (frames ok), `0x63` gives 199 (frame exhaustion proof), and `0x64` gives 201 (memory init error). None collide with 33, 35, 95, 97, 99, 101, 103, 105, 124–127, 137, 161, 163, 177, 179, or 181.

## 10. Smoke cases (`scripts/smoke.sh`)

Each new case is its own Cargo feature and its own QEMU run. Every judge requires the exact status and a full-line match.

| Case | Status | What it checks | Line |
| --- | --- | --- | --- |
| `heap` | 193 | `Box<u64>` value, `Vec<u32>` with 1,000 pushes (sum 499,500), a formatted `String`. Then `try_alloc` at alignments 1, 8, 16, 64, 4096: each address is a multiple of its alignment, inside the heap, and after the previous allocation. `align = 8192` returns `None` and leaves `used` unchanged. | `R3ATH3_HEAP ok size=1048576 used=<n> align_max=4096`; judge also requires 0 < `used` ≤ 1,048,576 |
| `heap-oom` | 195 | With nothing else allocated, 64 KiB chunks (align 4096) until `None`. Exactly 16 succeed. Then `used == 1048576` and a 1-byte `try_alloc` returns `None` with `used` unchanged. | `R3ATH3_HEAP_OOM ok chunks=16 used=1048576` |
| `heap-oom-panic` | 35 | `Vec::<u8>::with_capacity(2 * 1024 * 1024)` | `R3ATH3_PANIC ... message=memory allocation of 2097152 bytes failed` |
| `frames` | 197 | One frame: 4 KiB-aligned, ≥ `__kernel_end`, ≥ 1 MiB, below the top of usable RAM, all zero. Write and read back a pattern. Free it, and the next alloc returns the same frame, zeroed. A second free is `NotAllocated`. An unaligned input returns `Unaligned`, an out-of-range input returns `OutOfRange`, and frame 0 and the first kernel frame return `NotAllocated`. | `R3ATH3_FRAMES ok total=<n> free=<n> first=0x<hex> kernel_end=0x<hex>`; judge also requires `first >= kernel_end` and `total * 4096 <= 67108864` |
| `frames-oom` | 199 | Allocate until `None`. The count equals `free` at init, the highest frame plus 4096 is at most the top of usable RAM, and a further alloc is still `None`. Free one frame, and the next alloc returns it. Timeout 20 s, since zeroing about 64 MiB under TCG is slower. | `R3ATH3_FRAMES_OOM ok allocated=<n> max=0x<hex>`; judge also requires `max + 4096 <= 67108864` |
| `mem-badinfo` | 201 | The feature zeroes the saved `pvh_start_info` (through a test hook in `memmap.rs`) before `mem::init()`. | `R3ATH3_MEM_ERROR reason=startinfo` |

A failed check inside a test body panics, which exits 35 and prints no case line, so the judge fails.

The harness self-test also feeds each new judge a right line with a wrong status and a right status with a wrong line, and requires both to fail.

`./scripts/smoke.sh` runs 27 cases: the 21 earlier ones plus these six. It logs the git head and the number of dirty entries before and after the run, and the toolchain versions.

## 11. Unsafe rule

`unsafe` stays in driver files. The three new ones are:

- `memmap.rs`: raw reads of the PVH start-info and memory-map table, the extern `pvh_start_info` symbol, and the table writes in `init`.
- `heap.rs`: the `unsafe impl GlobalAlloc` and the `unsafe impl Sync` for the static area.
- `frames.rs`: the shared bitmap state behind the busy flag, `rep stosq` zeroing of a frame, and the volatile frame reads and writes in the two test helpers.

The only other change is the one `movl %ebx, pvh_start_info` line (and its symbol) in the assembly in `boot.rs`. `mem.rs`, `memtest.rs`, and `main.rs` have no `unsafe`. Every new `unsafe` block or impl has a `// SAFETY:` comment naming its invariant.
