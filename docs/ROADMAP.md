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

## Next, in order
10. **Storage.** A RAM disk first, then NVMe, so the audit log and saved data survive a reboot.
   In progress: docs/STORAGE.md (part A: RAM disk).
11. **WebAssembly runtime.** A sandbox for apps and for AI tools, running under the gate's permissions. Game logic is written once and runs on R3@TH3 and on the web.
    - The first native app is Glue Duty.
    - Then Watch Mechanic, The Movement (a system monitor), the shop terminal, and a Patterns trainer.
12. **Framebuffer graphics + PC speaker sound.**
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
