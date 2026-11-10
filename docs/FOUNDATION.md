# Boot foundation

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

The image in this repository is a QEMU-only x86_64 boot milestone. It prints one serial line and exits. It is not a cooperative OS.

## Docs repository

`https://github.com/R3ATH3-OS/r3ath3-docs` describes interaction, agent, model, service, and execution layers whose execution layer is Windows, WSL, Docker, and Python. Those notes stay there. Nothing here rewrites them or pretends the kernel is that system.

## Implemented

| Piece | Behavior |
| --- | --- |
| Boot | PVH `PHYS32_ENTRY` note, 32-bit entry, 1 GiB identity map, 64-bit `kernel_main` |
| Serial | Polled COM1, fixed boot line |
| VGA text | Same line at `0xB8000` for a screenshot |
| Panic | Serial `R3ATH3_PANIC` plus message, then QEMU fail status 35 |
| Smoke test | Pass (33), panic (35), hang (timeout 124), harness rejects a missing marker |

## Planned, not in this image

Exception reporting, an allocator, processes, isolation, a scheduler, drivers beyond COM1 and VGA text, networking, persistent storage, a human authorization gate, an append-only action record, and any model or agent runtime. Those do not belong in ring 0 by default. A future action gate, if built, stays outside the kernel and starts from denial.

## Human–AI rule

A model suggestion is untrusted input. This kernel has no interface that turns a suggestion into a privileged action. Denial is the only behavior available today, because no action interface exists. A later gate must be human-controlled, limited to an explicit capability, and auditable. This milestone does not implement that gate and does not log actions.

## Unsafe boundaries

All `unsafe` is in `kernel/src/port.rs`, `kernel/src/serial.rs`, `kernel/src/vga.rs`, `kernel/src/qemu_exit.rs`, and the assembly in `kernel/src/boot.rs`.

- Port I/O is limited to COM1 `0x3F8..=0x3FD` and QEMU debug-exit `0xF4`.
- Interrupts stay off. One CPU. No shared device state.
- VGA writes are 80×25 cells at `0xB8000` after the identity map is on.
- The assembly runs only as the firmware/QEMU entry, builds the page tables before paging, and calls `kernel_main` once on a 16 KiB stack.
- There is no IDT. A fault before the serial driver is a QEMU reset/exit without a Rust panic line.
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
