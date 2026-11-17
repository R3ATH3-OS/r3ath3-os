# Hardware boot (milestone 13, part A)

**Vision:** An authentic Human–AI co-op OS.

Part A makes one image that boots the way a PC boots: GRUB, a Multiboot2 header, and a text console on the firmware framebuffer. Part B (USB keyboard) is not in this image. Part C is the checklist at the end of this file. A builder does not perform Part C.

The QEMU `-kernel` path is unchanged. It still enters through the PVH note. The 79 existing smoke cases keep their statuses, logs, and goldens.

## What this image does

The ELF has two 32-bit entries. `_start` is the PVH entry and the ELF `e_entry`. `_start_mb2` is the Multiboot2 entry. The header's entry-address tag points at `_start_mb2`. QEMU's own Multiboot loader still rejects this ELF64 file, which is why `-kernel` stays on PVH.

`scripts/make-image.sh` builds the kernel with `shell,glue-baked,hw` and runs `grub-mkrescue`. The output is the regular file `build/image/r3ath3-usb.img`. The script refuses a path under `/dev` and refuses a block device. It prints the SHA-256 and the size in bytes. It does not write a stick.

`scripts/grub.cfg` is the one menu entry. Timeout is 0. It loads `all_video` before `multiboot2 /boot/r3ath3`, because without that module OVMF GRUB finds no video mode and the kernel gets no framebuffer tag. The config does not attach GRUB's serial terminal. The kernel's own COM1 init is enough, and a GRUB serial terminal would prefix the first kernel line.

## Boot info

`_start_mb2` requires magic `0x36D76289` in `%eax` and an info pointer in `%ebx`. `mb2::parse` reads that block once. The block must be 8-byte aligned, 8 to 64 KiB, and entirely below 1 GiB. Tags are 8-byte aligned. A bad block exits 201 with `R3ATH3_MEM_ERROR reason=mb2`.

Type 1 memory-map entries are RAM and go into the same table `memmap` already uses. At most 32 regions are stored. Type 1 entries are kept first. If a type 1 entry does not fit, init fails with `mb2`.

The framebuffer tag is recorded as address, pitch, width, height, and bits per pixel. The linear console uses it only when the type is RGB and the depth is 32. An EGA text tag, or no tag, leaves the console on `0xB8000`.

EFI tags 11, 12, 17, 19, and 20 mean `fw=uefi`. Any other Multiboot2 boot is `fw=bios`. A `-kernel` boot has no Multiboot2 block, so the hw banner says `fw=pvh`.

## Screen

`lfb.rs` draws an 80×25 console of 8×16 cells, 640×400, centered on the framebuffer. Ink is green `0x55FF55` on black. The cursor is an underline on the bottom row of the cell. Clear, scroll, backspace, and the deferred wrap match `vga.rs`. `vga`'s public console calls go there when a linear framebuffer was accepted.

The font is Spleen 8x16 2.2.0, Copyright (c) 2018-2026 Frederic Cambus, BSD-2-Clause. The bytes and the license text are in `kernel/src/font8x16.rs`. Bit 7 is the leftmost pixel.

On a PVH boot nothing here runs. `0xB8000` and VGA mode 13h stay as they are.

An app that exports `r3_pixels`, including Glue Duty, prints `app NAME: no vga screen on this boot` when the linear console is active, and stays text. The text Glue Duty path is unchanged. The PC speaker is unchanged.

## Memory

The boot stub still identity-maps the first 1 GiB with 2 MiB pages. The 2 MiB page at `0x3000_0000` stays uncached. One spare page directory, `pd_hi`, is zeroed and unused until a framebuffer above 1 GiB needs it. `paging::map_uncached_2m` fills that directory and one PDPT slot other than slot 0, then reloads CR3. There is no general page-table manager.

The frame allocator excludes `0x3000_0000..0x3020_0000` in every build, including PVH. At `-m 64M` that window is not RAM, so `frames_total` does not change because of the exclusion. On a `-m 2G` PVH boot the same window is inside a type-1 region. Before the exclusion the allocator managed it. After the exclusion it does not, and the managed count drops by 512 frames, which is the 2 MiB window.

The shell at `-m 64M` reports `frames_total=15746`, inside the published window 15741..=15757. `heap_used` stays at most 4096.

## The `hw` build

`hw` is a feature, not a test mode, so it combines with `shell`. On that build:

- `pci` and `nvme` are not compiled. The shell mounts the RAM disk only. A reboot erases it.
- Nothing writes port `0xF4`. After a status the kernel prints `R3ATH3_HALT status=<n>` on the screen and on serial, then halts.
- `pci.rs` does not move any BAR. Part A does not look up an xHCI controller.
- After the welcome line the shell prints `R3ATH3 hw boot fw=<bios|uefi|pvh> fb=<WxH|text> mem_mib=<n>`.

## Smoke

Four cases, on top of the existing 79. The build count is 39.

| Case | Status | How it runs |
| --- | --- | --- |
| `mb2-bios` | 207 | `mb2-boot` kernel, GRUB ISO, SeaBIOS, `-m 64M` |
| `mb2-uefi` | 207 | Same ISO, OVMF pflash, `-m 2G`, so the uncached window is inside RAM |
| `lfb-console` | 223 | `lfb-console` kernel, OVMF. FNV-1a of the 640×400 console is `434d640f42624b08`. A glyph sample matches the font. |
| `hw-image` | none | `build/image/r3ath3-usb.img` as a USB stick on OVMF, `-m 256M`. No debug-exit device. Pass needs the hw boot line and `r3> `, then a monitor `quit`. |

`hw-image` is the accepted exception to "every feature gets its own exit status". The `hw` image has no exit port, because that port is an emulator device. A triple fault cannot pass: the prompt is missing. The case prints `hw_image_boot_ms=<n>`, which is not judged.

A swap check feeds the Multiboot2 log to the console judge and the console log to the Multiboot2 judge. Both must fail.

Help stays 24 lines. There is no new shell verb. `selftest` stays 8 checks.

After part A the unused odd statuses are 237, 239, and 255. 237 is reserved for the USB keyboard proof and is not used by this image. 255 is not used.

## QEMU

BIOS, image as a CD. The file is a hybrid ISO. The shell stays at `r3> ` because this image does not write port `0xF4`.

```sh
qemu-system-x86_64 -machine pc,accel=tcg -cpu qemu64 -m 64M -smp 1 \
  -cdrom build/image/r3ath3-usb.img -boot d \
  -nic none -display none -vga std -serial stdio -no-reboot
```

UEFI, image as a USB stick. Copy the VARS file first. Do not edit the package file in place.

```sh
cp /usr/share/OVMF/OVMF_VARS_4M.fd /tmp/r3ath3-ovmf-vars.fd
qemu-system-x86_64 -machine q35,accel=tcg -cpu qemu64 -m 256M -smp 1 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=/tmp/r3ath3-ovmf-vars.fd \
  -device qemu-xhci,id=xhci \
  -drive if=none,id=stick,format=raw,file=build/image/r3ath3-usb.img \
  -device usb-storage,bus=xhci.0,drive=stick \
  -nic none -display none -serial stdio -no-reboot
```

On this QEMU 8.2.2 the UEFI boot printed `fw=uefi fb=1024x768` at framebuffer `0x80000000`. The BIOS boot printed `fw=bios fb=1024x768` at `0xfd000000`. Both are above 1 GiB. SeaBIOS can also hand the kernel an EGA text tag; the console then stays at `0xB8000`.

## Unsafe

New privileged code is in `boot.rs`, `mb2.rs`, `paging.rs`, `lfb.rs`, and one table write in `memmap.rs`. `hwtest.rs` and `font8x16.rs` have none. `pci.rs` is unchanged. Every new block has a `// SAFETY:` comment. `rg -n 'unsafe|asm!' kernel/src` is 166 lines. It was 145. The 21 added lines are listed in the part A pull request.

## Packages

`.cursor/environment.json` installs `grub-pc-bin`, `grub-efi-amd64-bin`, `xorriso`, `mtools`, and `ovmf` from apt, beside the existing `qemu-system-x86`. rustc stays 1.83.0. There is no crates.io dependency and no new Rust target. GRUB is GPL-3.0. It is a build tool. The ISO contains it. The kernel source does not vendor GRUB.

## Part C: real machine bring-up

None of these steps is done by a builder without John's explicit go for that step. Each one is a separate yes. This file records the checklist. It does not perform it.

1. **Proxmox VM (no purchase, no bare metal).** On John's Proxmox host, create a VM with OVMF (UEFI), q35, 512 MiB RAM, an xHCI USB controller, and the image attached as a USB disk. Boot it and read the `hw boot` line. Touching the Proxmox host is a homelab change, so John approves it.
2. **Pick the machine.** Buying a box is John's call. Criteria: x86_64, UEFI with Secure Boot that can be turned off, USB boot, at least one USB port wired straight to the controller. A PS/2 port or a COM header helps debugging. Using an existing PC is possible, because the `hw` image never touches its disks, but it means changing that PC's firmware settings.
3. **Write the USB stick.** Write `r3ath3-usb.img` to a stick that holds nothing needed, after checking the SHA-256 that `make-image.sh` printed. The target drive is named and confirmed before writing. Choosing the wrong drive destroys its contents.
4. **Firmware settings.** Turn off Secure Boot. The image is not signed. If the firmware offers fast boot, note that setting. Note whether legacy USB support or CSM exist.
5. **Boot and record.** Power on with the stick in. Record the `hw boot` line, whether the PS/2 path typed, whether `uptime` advances, and the seconds to the prompt.
6. **USB keyboard on the box.** This step waits until part B is merged. Repeat the boot with the keyboard on each port, and note which ports work.
7. **Results** go into this file in a small docs-only change that John merges.

If step 5 shows no ticks, no keyboard at all, or a load failure at 1 MiB, stop. That is a new design, not a quick fix.
