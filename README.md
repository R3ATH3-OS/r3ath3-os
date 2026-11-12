# r3ath3-os
R3@TH3 OS – a human–AI communication operating environment.

Suggested wording, not a certification and not a claim that the system exists yet: **An authentic Human–AI co-op OS**.

This repository boots a small Rust kernel in QEMU. The default image prints a boot line and exits. The `shell` feature reads the keyboard and answers agent requests on COM1. The model stays on the host. The kernel does not run a model.

## What is implemented

- x86_64 kernel, ring 0 only, built with `no_std` / `no_main` (not a Linux wrapper).
- QEMU `-kernel` entry via a Xen PVH `PHYS32_ENTRY` note. QEMU 8.2 rejects ELF64 Multiboot, so there is no Multiboot header. The entry contract is 32-bit protected mode, paging off.
- Identity map of the first 1 GiB, then a jump to 64-bit Rust.
- Deterministic COM1 line: `R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation`
- QEMU `isa-debug-exit`: pass status **33**, panic status **35**.
- A 64-bit IDT for CPU exceptions, plus a GDT/TSS so `#DF` uses a dedicated IST stack. Init remaps the 8259 PIC and runs `sti`, with only IRQ0 (timer) and IRQ1 (keyboard) unmasked.
- Fault serial line, then a distinct QEMU status: `#UD` **99**, `#PF` **103** (`cr2=0x40000000`), `#DF` **105**. Also handled: `#DE` **97**, `#GP` **101**. Any other vector exits **95** (outside GNU timeout's 124–127). The RIP printed for `#DF` is architecturally undefined. The `#DF` stack has no guard page.
- Bounded smoke test of 56 cases: pass, injected panic, hang/timeout, `#UD`, `#PF`, forced `#DF`, 12 task-script cases, timer, keyboard, keyboard script, the six allocator cases, twenty-one shell and gate cases, six scheduler proofs, and two gate proofs. The harness rejects a missing marker, a silent log, and a triple-fault status.
- A fixed-buffer task language. The `script` build reads COM1 (status **161**/**163**). The `kbd-script` build reads the PS/2 keyboard and echoes lines (success status **181**). Spec: `docs/TASK.md`.
- IRQ0 timer at nominal 100 Hz (PIT divisor 11932) and IRQ1 keyboard. Other PIC lines stay masked. Proof waits 300 ticks and exits **177**. Keyboard echo exits **179**.
- Memory (milestone 5, spec `docs/MEMORY.md`): the boot stub saves the PVH start-info pointer, `memmap` parses the memory map, `heap` is a 1 MiB bump allocator registered as `#[global_allocator]` (`Box`, `Vec`, `String` work; the heap only grows and is reclaimed only by reboot), and `frames` hands out zeroed 4 KiB frames from usable RAM in [1 MiB, 1 GiB). Bad start info prints `R3ATH3_MEM_ERROR reason=<startinfo|memmap|nousable>` and exits **201**. Smoke statuses: heap **193**, heap exhaustion **195**, frames **197**, frame exhaustion **199**. A heap overflow through `Box`/`Vec`/`String` panics and exits **35**.
- Interactive shell (milestone 6, spec `docs/SHELL.md`): prompt `r3> `, PS/2 line editing, and the built-ins `help`, `echo`, `clear`, `uptime`, `mem`, `ps`, `spawn`, `wait`, `badges`, `grant`, `revoke`, `approve`, `deny`, `run`, `exit`, and `halt`. COM1 is the judged transcript and the agent input. The VGA text console shows the same text except `R3ATH3_*` and `R3GATE` lines. Statuses: clean exit **209**, command error **211**, line overflow **213**, script error **215**, halt **217**.
- Scheduler (milestone 7, spec `docs/SCHEDULER.md`): eight task slots, cooperative yield and one-tick preemption, sleep, wait, and reap. A waiter reaps on wake only for the incarnation it waited for. Statuses: cooperative **225**, preemptive **227**, sleep **229**, full table **231**, exit and reap **233**, wait-race **235**.
- Human gate (milestone 8, spec `docs/GATE.md`): up to four badges. `read` and `say` need the granted permission. `spawn` and `halt` also need a human `approve N` of that request. One privileged request waits, and an unanswered request is denied after 60 seconds. Statuses: gate core **241**, gate expiry **243**.

## What is not implemented

Userspace, processes, isolation, user mode, virtual memory management (page tables are only the boot identity map), a heap that can free or grow, files, networking, accounts, model inference, an agent runtime, an audit log, a mouse, a graphical UI, shell history, and tab completion. Only IRQ0 and IRQ1 are unmasked. The task language is not an AI runtime. AI code is not linked into this kernel. The gate checks permissions of request lines that arrive on COM1 (`docs/GATE.md`). Agent names are self-asserted.

Historical notes in [r3ath3-docs](https://github.com/R3ATH3-OS/r3ath3-docs) describe a userspace agent environment on Windows, WSL, and Docker. This boot milestone does not migrate or replace those notes.

## Human–AI boundary

A model suggestion is untrusted input. The model runs outside the kernel and reaches it only through the gate on COM1. The gate is enforced inside the kernel, starts from denial, is controlled by the human at the keyboard, and limits each agent to the permissions on its badge. Every privileged action needs a human approval of that request. Milestone 8 prints every gate outcome; the append-only record is milestone 9. See `docs/FOUNDATION.md` and `docs/GATE.md`.

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
