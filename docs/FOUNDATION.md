# Boot foundation

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

The image in this repository is a QEMU-only x86_64 kernel. It boots, reports CPU exceptions, and can run one fixed-buffer task script from COM1. It is not a cooperative OS.

## Docs repository

`https://github.com/R3ATH3-OS/r3ath3-docs` describes interaction, agent, model, service, and execution layers whose execution layer is Windows, WSL, Docker, and Python. Those notes stay there. Nothing here rewrites them or pretends the kernel is that system.

## Implemented

| Piece | Behavior |
| --- | --- |
| Boot | PVH `PHYS32_ENTRY` note, 32-bit entry, 1 GiB identity map of 2 MiB pages, 64-bit `kernel_main`. The page at `0x3000_0000` is uncached (PWT and PCD) and is the NVMe MMIO window. With `-m 64M` it is not RAM. |
| Serial | Polled COM1, fixed boot line |
| VGA text | 80×25 green-on-black console at `0xB8000`: cursor tracking, deferred wrap, scrolling, clear, backspace, and a hardware cursor on CRTC ports `0x3D4`/`0x3D5`. `write_marker` still fills the screen with the boot line. With `-vga none`, writes are discarded. |
| Panic | Serial `R3ATH3_PANIC` plus message, then QEMU fail status 35 |
| CPU exceptions | IDT for vectors 0–31. Named lines for `#DE`, `#UD`, `#GP`, `#PF`, and `#DF`. `#DF` uses IST1, a 16 KiB stack with no guard page. The RIP saved for `#DF` is architecturally undefined; the line prints the value the CPU pushed. AMD-only `#VC` (29) and `#SX` (30) keep the CPU error code. Line shape: `R3ATH3_FAULT vector=<n> name=<name> error=<code or none> rip=<hex>` and `cr2=<hex>` on `#PF`. Named QEMU statuses are 97/99/101/103/105. Any other vector exits 95, outside GNU timeout's 124–127. |
| Smoke test | 68 cases and 28 builds. Pass (33), panic (35), hang (timeout 124), `#UD` (99), `#PF` (103), `#DF` (105), script ok (161), script error (163, 11 cases), timer (177), keys (179), keyboard script (181), heap (193), heap exhaustion (195), heap overflow panic (35), frames (197), frame exhaustion (199), bad start info (201), the shell cases (clean exit 209, command error 211, line overflow 213, script error 215, halt 217), the scheduler proofs: cooperative (225), preemptive (227), sleep (229), full table (231), exit/reap (233), and the wait-race proof (235), the gate proofs: `gate-core` (241) and `gate-expire` (243), the recorder proof `recorder-core` (245), the store proof `store-core` (247), and the NVMe proof `nvme-core` (249). A status names an outcome. A feature that can end its own run gets its own status; shell commands are judged by exact transcript under the shell's outcome statuses, and a swap self-test proves those cases cannot pass for each other. 209 covers sixteen, 211 covers nine, 213 covers two, and 217 covers two. Fault and allocator judges require the exact status and a full line. Script, shell, scheduler, gate, recorder, and store judges require the whole serial log. `log-record` and `log-full` mask ticks, links, and heads, then recompute them. `store-ram` does the same and checks that boot 1 matches the live record. `store-reboot` and `store-tamper` check the second boot against the first. `shell-screen` also compares the VGA buffer. |
| Memory | The boot stub saves the PVH start-info pointer. `memmap` validates it and copies the memory map. `heap` is a static 1 MiB bump allocator (`#[global_allocator]`, alignment 1–4096, `dealloc` is a no-op, the heap only grows). `frames` is a bitmap allocator of zeroed 4 KiB frames from type-1 RAM in [1 MiB, 1 GiB), excluding the kernel image and the start-info pages. Memory init runs in every build and prints nothing on success. A bad start info or memory map prints `R3ATH3_MEM_ERROR reason=<startinfo\|memmap\|nousable>` and exits 201. IRQ handlers must not allocate. Spec: `docs/MEMORY.md`. |
| Task language | Fixed buffers only. `script` reads polled COM1 (status 161/163). `kbd-script` reads IRQ1 and echoes lines (success 181). Commands and limits are in `docs/TASK.md`. Errors are `Result` values. `script.rs` has no `unsafe`. |
| Shell | `shell` feature: prompt `r3> `, PS/2 line editing, and built-ins `help`, `echo`, `clear`, `uptime`, `mem`, `ps`, `spawn`, `wait`, `badges`, `grant`, `revoke`, `approve`, `deny`, `log`, `replay`, `selftest`, `store`, `boots`, `save`, `show`, `forget`, `run`, `exit`, `halt`. COM1 is judged and is the agent input. VGA shows the same text except `R3ATH3_*` and `R3GATE` lines. Statuses 209, 211, 213, 215, and 217. Spec: `docs/SHELL.md`. |
| Scheduler | Eight task slots, round-robin, cooperative yield and one-tick PIT preemption. States ready, running, sleeping, blocked, and exited. Idle halts when nothing else is ready. Seven 16 KiB stacks in `.bss` with a canary. A waiter reaps on wake only if the slot still holds the generation it waited for. Spec: `docs/SCHEDULER.md`. Statuses 225, 227, 229, 231, 233, and 235. |
| Gate | Up to four badges, verbs `read`, `say`, `spawn`, and `halt`. `read` and `say` need the permission. `spawn` and `halt` also need approval of that request. One privileged request waits; an unanswered request is denied after 60 seconds. The human types `approve N` or `deny N`. Spec: `docs/GATE.md`. Statuses 241 and 243. |
| Flight recorder | An append-only record of 128 entries in kernel memory, with a 64-bit FNV-1a hash chain. The shell appends every gate outcome and every human command, and writes each one through to the store. `log` and `replay` print it. `replay` verifies and does not re-execute. `selftest` runs eight checks and prints a report card. With the QEMU-emulated NVMe image, earlier boots stay in the host file. Without a disk the store is the RAM disk, and a reboot erases it. The live record starts empty every boot. This is not real hardware. Spec: `docs/RECORDER.md`. Status 245. |
| Store | A 512-block RAM disk in a module-private `.bss` `IrqCell`, holding the `R3ATH3ST` version 1 format. Five human commands: `store`, `boots`, `save`, `show`, `forget`. Each recorded action is written through. With no NVMe controller a reboot erases the disk. Spec: `docs/STORAGE.md`. Status 247. |
| NVMe | A polled driver for QEMU's emulated NVMe controller (`pci.rs`, `nvme.rs`). BAR0 is placed at `0x3000_0000`. Config cycles use ports `0xCF8` and `0xCFC`. With the image attached, earlier boots stay in the host file and are read with `boots K`. The live record still starts empty. Without a controller the shell uses the RAM disk. This is not real hardware. Status 249. |
| Timer and keyboard | 8259 PIC vectors 32–47, only IRQ0 and IRQ1 unmasked. PIT divisor 11932, nominal 100 Hz. 300 ticks must move the virtual RTC by 3 or 4 seconds (status 177). PS/2 scan code set 1, 128-event ring, overflow drops. Echo test status 179. |

## Planned, not in this image

A heap that frees or grows, a free-list or slab allocator, page-table management, guard pages, processes, isolation, user mode, networking, persistent storage on real hardware (this image keeps earlier boots only in a QEMU-emulated NVMe image; the RAM disk is still erased by a reboot), a file system, a mouse, a graphical UI, shell history, tab completion, and any model or agent runtime. IRQ0 and IRQ1 are the only unmasked lines. There is no APIC.

## Human–AI rule

A model suggestion is untrusted input. The model runs outside the kernel and reaches it only through the gate on COM1. The gate is enforced inside the kernel, starts from denial, is controlled by the human at the keyboard, and limits each agent to the permissions on its badge. Every privileged action needs a human approval of that request. Every gate outcome and every human command is appended to the flight recorder, an append-only record in kernel memory that the human reads with `log` and `replay`. Each entry is written through to the store. With QEMU's emulated NVMe disk, earlier boots are read with `boots K`. Without a disk the store is the RAM disk, and a reboot erases both. The live record starts empty every boot.

## Unsafe boundaries

All `unsafe` is in `kernel/src/port.rs`, `kernel/src/serial.rs`, `kernel/src/vga.rs`, `kernel/src/qemu_exit.rs`, `kernel/src/irq.rs`, `kernel/src/exceptions.rs`, `kernel/src/memmap.rs`, `kernel/src/heap.rs`, `kernel/src/frames.rs`, `kernel/src/context.rs`, `kernel/src/pci.rs`, `kernel/src/nvme.rs`, `kernel/src/wasm/arena.rs`, and the assembly in `kernel/src/boot.rs`. `script.rs`, `shell.rs`, `sched.rs`, `schedtest.rs`, `gate.rs`, `gatetest.rs`, `recorder.rs`, `recordertest.rs`, `selftest.rs`, `block.rs`, `ramdisk.rs`, `store.rs`, `storetest.rs`, `nvmetest.rs`, `mem.rs`, `memtest.rs`, `main.rs`, `apps.rs`, `wasmtest.rs`, and the other files under `kernel/src/wasm/` call those drivers and do not add their own `unsafe`. `wasm/arena.rs` adds two lines: `unsafe impl Sync` for the arena cell, and the one borrow inside `with_arena`. Each `unsafe` block added for the allocator has a `// SAFETY:` comment, as do the VGA console state, its volatile cell accesses, the CRTC port writes, `irq::idle_hlt`, `irq::without_interrupts`, and every block in `context.rs`.

- `context.rs` owns the task-switch `global_asm!`, the trampoline, the static stack pool and canaries, the saved-RSP table, the initial frames, and `IrqCell<T>`. One CPU. Every `IrqCell` access runs with IF clear, so the timer hook and task code never overlap. The switch saves and restores six callee-saved registers and does not touch the FPU.

- `memmap.rs` reads the PVH `hvm_start_info` struct and the memory-map table with volatile or unaligned loads, only at addresses already checked to be below 1 GiB (identity mapped). It reads the `pvh_start_info` symbol the boot stub writes, and fills a static table once, before it is published.
- `heap.rs` owns a static 1 MiB `.bss` area and the `GlobalAlloc` impl. Ranges are reserved by compare-exchange on one atomic offset and never handed out twice. `dealloc` does nothing.
- `frames.rs` owns the two frame bitmaps behind a busy flag (reentry panics; it also reads the addresses of the linker symbols `__kernel_start` and `__kernel_end`, which needs no `unsafe`), zeroes a frame it just took with `rep stosq` through the identity map, and does volatile reads and writes of an allocated frame in two test helpers.

- Port I/O is COM1 `0x3F8..=0x3FD`, the PIC (`0x20`/`0x21`/`0xA0`/`0xA1`), the PIT (`0x40`/`0x43`), the PS/2 controller (`0x60`/`0x64`), the CMOS RTC (`0x70`/`0x71`), the VGA CRTC (`0x3D4`/`0x3D5`), and QEMU debug-exit `0xF4`.
- One CPU. After init, IRQ0 and IRQ1 are unmasked and `sti` has run. Other PIC inputs stay masked. The tick counter and the keyboard ring are shared with those handlers: IRQ0 updates the counter, IRQ1 is the only producer of the ring, and the only consumer is `pop_key` (the boot context, which in a `sched` build is the shell task). A full ring drops the event and increments a counter. Exception and IRQ gates clear IF while a handler runs. In a `sched` build the IRQ0 handler also calls `sched::on_tick` after EOI.
- VGA writes are 80×25 cells at `0xB8000` after the identity map is on. The console cursor is touched only by the shell task, through `vga::with_console`. IRQ handlers and other tasks do not touch VGA. A preemption in the middle of a console call is harmless because no other task enters that module.
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
