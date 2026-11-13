# R3@TH3 OS Roadmap

**Vision:** An authentic Human–AI co-op OS.

**Headline:** The AI is a crew member with a badge. It can't touch the machine without a human's approval at the gate, and everything is recorded.

## Rules for every milestone
- Docs are the spec. Each milestone starts with a design doc.
- One bounded milestone at a time: one build, one clean-tree smoke run at the exact head, one focused review, and then John decides on the merge.
- A status names an outcome. A feature that can end its own run gets its own status; shell commands are judged by exact transcript under the shell's outcome statuses, and a swap self-test proves those cases cannot pass for each other.
- Unsafe code is allowed only in driver files.
- No speculative expansion.

## Done
1. Boot foundation (PVH, serial, isa-debug-exit)
2. Fault handling
3. Task-language interpreter
4. PIC/PIT timer + PS/2 keyboard
5. Heap + frame allocator
6. Interactive shell. Merged in PR #6 at 4de4b2a (36/36 smoke cases).
7. Scheduler. Merged in PR #7 at f1d2a3d (46/46 smoke cases).
8. Human gate. Merged in PR #8 at 64d7225 from tested head 92abe8c (56/56 smoke cases).
9. Flight recorder + selftest. Merged in PR #9 at 08a4bc6 from tested head 9ce5596 (61/61 smoke cases).
10. **Storage (QEMU-emulated).** RAM disk merged in PR #10; emulated NVMe merged in PR #11 at `3ac0fe5` from tested head `ac0f38a`. Official smoke on the tested head `ac0f38a` was 68/68, 28 builds, harness-selftest PASS, smoke_exit=0, dirty 0, rustc 1.83.0, QEMU 8.2.2. Saved data survives a reboot only inside the emulated disk image. Without a disk the RAM disk is erased by a reboot.
11. **WebAssembly runtime.** A sandbox for apps and for AI tools, running under the gate's permissions. Game logic is written once and runs on R3@TH3 and on the web. Merged in PR #13 at `3036b6b` from tested head `c9c778f`. Official smoke on that tested head was 75/75. Glue Duty runs in text mode. Later native apps, not started: Watch Mechanic, The Movement (a system monitor), the shop terminal, and a Patterns trainer.

## Next, in order
12. **Framebuffer graphics + PC speaker sound.** Part A is on main at `43b99218`, the merge of PR #14, tested head `918fd40`. Official smoke on that tested head was 77/77, 34 builds. It is VGA mode 13h, 320×200×8, and the PC speaker on PIT channel 2 (`docs/FRAMEBUFFER.md`, `docs/SPEAKER.md`). Part B ports Glue Duty onto that framebuffer and that speaker. The two-host match is the 320×200×8 buffer. Higher resolution and other upgrades come later.
13. **Real hardware.** USB keyboard and booting on a mini PC or a Raspberry Pi, leading to the instant-boot, offline shop appliance.

## Editions (later)
- **Shop:** for maintenance work.
- **Station:** local AI and homelab.
- **Bench:** for makers and developers.
- **Play:** native games.

All of them share one kernel.

## Not in scope (honest limits)
- Running Unreal, a web browser, or GPU drivers.
- Running the AI model inside the kernel. The model runs on the host and talks to the OS through the gate.
- A bare-metal hypervisor. That's a someday goal, after processes, memory protection, and drivers. For now, Proxmox hosts R3@TH3 as a guest.
