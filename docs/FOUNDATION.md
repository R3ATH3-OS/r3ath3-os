# Boot foundation

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

The image in this repository is a QEMU-only x86_64 kernel. It boots, reports CPU exceptions, and can run one fixed-buffer task script from COM1. It is not a cooperative OS.

## Docs repository

`https://github.com/R3ATH3-OS/r3ath3-docs` describes interaction, agent, model, service, and execution layers whose execution layer is Windows, WSL, Docker, and Python. Those notes stay there. Nothing here rewrites them or pretends the kernel is that system.

## Implemented

| Piece | Behavior |
| --- | --- |
| Boot | PVH `PHYS32_ENTRY` note, 32-bit entry, 1 GiB identity map, 64-bit `kernel_main` |
| Serial | Polled COM1, fixed boot line |
| VGA text | 80×25 green-on-black console at `0xB8000`: cursor tracking, deferred wrap, scrolling, clear, backspace, and a hardware cursor on CRTC ports `0x3D4`/`0x3D5`. `write_marker` still fills the screen with the boot line. With `-vga none`, writes are discarded. |
| Panic | Serial `R3ATH3_PANIC` plus message, then QEMU fail status 35 |
| CPU exceptions | IDT for vectors 0–31. Named lines for `#DE`, `#UD`, `#GP`, `#PF`, and `#DF`. `#DF` uses IST1, a 16 KiB stack with no guard page. The RIP saved for `#DF` is architecturally undefined; the line prints the value the CPU pushed. AMD-only `#VC` (29) and `#SX` (30) keep the CPU error code. Line shape: `R3ATH3_FAULT vector=<n> name=<name> error=<code or none> rip=<hex>` and `cr2=<hex>` on `#PF`. Named QEMU statuses are 97/99/101/103/105. Any other vector exits 95, outside GNU timeout's 124–127. |
| Smoke test | 36 cases. Pass (33), panic (35), hang (timeout 124), `#UD` (99), `#PF` (103), `#DF` (105), script ok (161), script error (163, 11 cases), timer (177), keys (179), keyboard script (181), heap (193), heap exhaustion (195), heap overflow panic (35), frames (197), frame exhaustion (199), bad start info (201), and the shell cases: clean exit (209, five cases told apart by the transcript), command error (211), line overflow (213), script error (215), and halt (217). Fault and allocator judges require the exact status and a full line. Script and shell judges require the whole serial log. `shell-screen` also compares the VGA buffer. |
| Memory | The boot stub saves the PVH start-info pointer. `memmap` validates it and copies the memory map. `heap` is a static 1 MiB bump allocator (`#[global_allocator]`, alignment 1–4096, `dealloc` is a no-op, the heap only grows). `frames` is a bitmap allocator of zeroed 4 KiB frames from type-1 RAM in [1 MiB, 1 GiB), excluding the kernel image and the start-info pages. Memory init runs in every build and prints nothing on success. A bad start info or memory map prints `R3ATH3_MEM_ERROR reason=<startinfo\|memmap\|nousable>` and exits 201. IRQ handlers must not allocate. Spec: `docs/MEMORY.md`. |
| Task language | Fixed buffers only. `script` reads polled COM1 (status 161/163). `kbd-script` reads IRQ1 and echoes lines (success 181). Commands and limits are in `docs/TASK.md`. Errors are `Result` values. `script.rs` has no `unsafe`. |
| Shell | `shell` feature: prompt `r3> `, PS/2 line editing, and built-ins `help`, `echo`, `clear`, `uptime`, `mem`, `run`, `exit`, `halt`. COM1 is judged. VGA shows the same text except `R3ATH3_*` lines. Statuses 209, 211, 213, 215, and 217. Spec: `docs/SHELL.md`. |
| Timer and keyboard | 8259 PIC vectors 32–47, only IRQ0 and IRQ1 unmasked. PIT divisor 11932, nominal 100 Hz. 300 ticks must move the virtual RTC by 3 or 4 seconds (status 177). PS/2 scan code set 1, 128-event ring, overflow drops. Echo test status 179. |

## Planned, not in this image

A heap that frees or grows, a free-list or slab allocator, page-table management, guard pages, processes, isolation, a scheduler, networking, persistent storage, a file system, a mouse, a graphical UI, shell history, tab completion, a human authorization gate, an append-only action record, and any model or agent runtime. IRQ0 and IRQ1 are the only unmasked lines. There is no APIC. A future action gate, if built, stays outside the kernel and starts from denial.

## Human–AI rule

A model suggestion is untrusted input. This kernel has no interface that turns a suggestion into a privileged action. Denial is the only behavior available today, because no action interface exists. A later gate must be human-controlled, limited to an explicit capability, and auditable. This milestone does not implement that gate and does not log actions.

## Unsafe boundaries

All `unsafe` is in `kernel/src/port.rs`, `kernel/src/serial.rs`, `kernel/src/vga.rs`, `kernel/src/qemu_exit.rs`, `kernel/src/irq.rs`, `kernel/src/exceptions.rs`, `kernel/src/memmap.rs`, `kernel/src/heap.rs`, `kernel/src/frames.rs`, and the assembly in `kernel/src/boot.rs`. `script.rs`, `shell.rs`, `mem.rs`, `memtest.rs`, and `main.rs` call those drivers and do not add their own `unsafe`. Each `unsafe` block added for the allocator has a `// SAFETY:` comment, as do the VGA console state, its volatile cell accesses, the CRTC port writes, and `irq::idle_hlt`.

- `memmap.rs` reads the PVH `hvm_start_info` struct and the memory-map table with volatile or unaligned loads, only at addresses already checked to be below 1 GiB (identity mapped). It reads the `pvh_start_info` symbol the boot stub writes, and fills a static table once, before it is published.
- `heap.rs` owns a static 1 MiB `.bss` area and the `GlobalAlloc` impl. Ranges are reserved by compare-exchange on one atomic offset and never handed out twice. `dealloc` does nothing.
- `frames.rs` owns the two frame bitmaps behind a busy flag (reentry panics; it also reads the addresses of the linker symbols `__kernel_start` and `__kernel_end`, which needs no `unsafe`), zeroes a frame it just took with `rep stosq` through the identity map, and does volatile reads and writes of an allocated frame in two test helpers.

- Port I/O is COM1 `0x3F8..=0x3FD`, the PIC (`0x20`/`0x21`/`0xA0`/`0xA1`), the PIT (`0x40`/`0x43`), the PS/2 controller (`0x60`/`0x64`), the CMOS RTC (`0x70`/`0x71`), the VGA CRTC (`0x3D4`/`0x3D5`), and QEMU debug-exit `0xF4`.
- One CPU. After init, IRQ0 and IRQ1 are unmasked and `sti` has run. Other PIC inputs stay masked. The tick counter and the keyboard ring are shared with those handlers: IRQ0 updates the counter, IRQ1 is the only producer of the ring, and main is the only consumer. A full ring drops the event and increments a counter. Exception and IRQ gates clear IF while a handler runs.
- VGA writes are 80×25 cells at `0xB8000` after the identity map is on. The console cursor is main-context state. IRQ handlers do not touch VGA.
- The boot stub saves `%ebx` (the PVH start-info pointer) in `pvh_start_info`, builds the page tables before paging, and calls `kernel_main` once on a 16 KiB stack. Its GDT has no TSS.
- `exceptions::init` then loads a GDT with a 64-bit TSS, points IST1 at a 16 KiB stack inside the identity map, and loads a 48-entry IDT (vectors 0–47). That IST stack has no guard page. `#DF` is reported only when the handler's stack pointer is inside it. The RIP in that report is architecturally undefined. A fault before that init is a QEMU exit without a fault line.
- The link keeps `.got`. `core`'s `panic_fmt` calls `rust_begin_unwind` through that slot. Dropping it makes a panic triple-fault with no serial line.
- `0xF4` is an emulator device. This image is not for physical hardware.

## Dependencies and licenses

| Component | Role | License |
| --- | --- | --- |
| This kernel's Rust and assembly | Project code | No repository `LICENSE` file. Choosing one is left to the maintainer. |
| rustc / cargo 1.83.0, target `x86_64-unknown-none` | Compiler and linker (`rust-lld`) | MIT OR Apache-2.0 |
| QEMU `qemu-system-x86_64` | Emulator only (TCG, no network device) | GPL-2.0-only |
| GNU binutils (`readelf` / `objdump` if used to inspect) | Optional inspection | GPL-3.0-or-later |

No crates.io crates are fetched. `kernel/Cargo.lock` records an empty dependency set. The PVH note is a 20-byte Xen ELF note (`PHYS32_ENTRY`) consumed by QEMU's `-kernel` loader. It is not vendored third-party source. QEMU 8.2's Multiboot loader rejects this ELF64 image, so the kernel does not use Multiboot.
