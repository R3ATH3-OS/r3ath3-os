# r3ath3-os
R3@TH3 OS – a human–AI communication operating environment.

Suggested wording, not a certification and not a claim that the system exists yet: **An authentic Human–AI co-op OS**.

This repository boots a small Rust kernel in QEMU and stops there. It does not run agents, models, or a cooperative userspace.

## What is implemented

- x86_64 kernel, ring 0 only, built with `no_std` / `no_main` (not a Linux wrapper).
- QEMU `-kernel` entry via a Xen PVH `PHYS32_ENTRY` note. QEMU 8.2 rejects ELF64 Multiboot, so there is no Multiboot header. The entry contract is 32-bit protected mode, paging off.
- Identity map of the first 1 GiB, then a jump to 64-bit Rust.
- Deterministic COM1 line: `R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation`
- QEMU `isa-debug-exit`: pass status **33**, panic status **35**.
- A 64-bit IDT for CPU exceptions, plus a GDT/TSS so `#DF` uses a dedicated IST stack. Maskable interrupts stay disabled. No PIC or APIC.
- Fault serial line, then a distinct QEMU status: `#UD` **99**, `#PF` **103** (`cr2=0x40000000`), `#DF` **105**. Also handled: `#DE` **97**, `#GP` **101**. Any other vector exits **95** (outside GNU timeout's 124–127). The RIP printed for `#DF` is architecturally undefined. The `#DF` stack has no guard page.
- Bounded smoke test: pass, injected panic, hang/timeout, `#UD`, `#PF`, and forced `#DF`. The harness rejects a missing marker, a silent log, and a triple-fault status.
- A fixed-buffer task language on COM1 in the `script` build. It prints `R3ATH3_OUT` lines, then `R3ATH3_SCRIPT_OK` (QEMU status **161**) or `R3ATH3_SCRIPT_ERROR` (status **163**). Spec: `docs/TASK.md`.

## What is not implemented

Userspace, processes, isolation, a scheduler, memory allocation, files, networking, accounts, model inference, agents, capability checks, and an audit log. Hardware IRQs, a timer, and `sti` are absent. The task language is not an AI runtime. AI code is not linked into this kernel.

Historical notes in [r3ath3-docs](https://github.com/R3ATH3-OS/r3ath3-docs) describe a userspace agent environment on Windows, WSL, and Docker. This boot milestone does not migrate or replace those notes.

## Human–AI boundary

AI output is untrusted data. This kernel cannot accept, authorize, or perform an action proposed by a model. Any later action layer must deny by default, require a human-controlled capability-limited authorization, and record the decision where a human can audit it. That layer is not in this image. See `docs/FOUNDATION.md`.

## Build and smoke test

Host tools this tree was brought up with: Rust **1.83.0** (`x86_64-unknown-none`) and QEMU **8.2.2** (`qemu-system-x86` `1:8.2.2+ds-0ubuntu1.18`, TCG). No crates.io dependencies.

```sh
rustup toolchain install 1.83.0 --profile minimal
rustup target add x86_64-unknown-none --toolchain 1.83.0
chmod +x scripts/smoke.sh scripts/capture-boot.sh
./scripts/smoke.sh
```

`scripts/smoke.sh` exits 0 on `SMOKE PASS` and non-zero on `SMOKE FAIL`. Serial logs and QEMU stderr land in `build/smoke/`.

Optional VGA capture (not the pass/fail test):

```sh
./scripts/capture-boot.sh
```

That writes `build/capture/boot.png` and `build/capture/serial.log`.

Manual boot, if you want the raw QEMU status:

```sh
cd kernel && cargo build --release
qemu-system-x86_64 -machine pc,accel=tcg -cpu qemu64 -m 64M -smp 1 \
  -kernel target/x86_64-unknown-none/release/r3ath3 \
  -nic none -display none -serial stdio -no-reboot \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04
```

Status 33 is the pass. Do not write this image to a physical disk or a host OS.

## Licenses

No third-party source is vendored. The kernel has zero crates.io dependencies. Tool licenses are in `docs/FOUNDATION.md`. This repository still has no project `LICENSE` file.
