# Framebuffer

Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".

Part A of milestone 12. The shell stays an 80×25 text shell. Graphics mode is only while a pixel path runs. Higher resolution is later, not this milestone. There is no mouse and no graphical shell. Glue Duty stays in text mode; pixels for that game are part B.

## What this image does

`kernel/src/fb.rs` is the driver. It is safe to call from the rest of the kernel. Every `unsafe` block in that file has a `// SAFETY:` comment.

Mode is VGA mode 13h: 320×200, 256 colors, one byte per pixel. The software buffer is 64,000 bytes in `.bss`, not on the heap. `blit` copies it to physical `0xA0000`. That address is inside the 1 GiB identity map, below 1 MiB, and the frame allocator never hands those frames out. A Bochs/VBE linear framebuffer is not programmed. It usually sits above the 1 GiB map and would need new page tables.

`fb` is internal. `shell` and `fb-core` turn it on. `fb-core` is a test mode. It is in the at-most-one assert and in both `boot_ok` exclusion lists.

## API

```rust
pub struct FbInfo {
    pub base: *mut u8,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: Format,
}
pub enum Format { Indexed8 }
pub fn info() -> FbInfo;
pub fn bytes() -> &'static mut [u8];
pub fn enter_graphics();
pub fn enter_text();
pub fn in_graphics() -> bool;
pub fn copy_from(src: &[u8]) -> bool;
pub fn blit();
```

`info` is the view milestone 13 can retarget. `base` points at the software buffer. For this backend, `width` is 320, `height` is 200, `stride` is 320, and `format` is `Indexed8`. `bytes` is that same buffer. `copy_from` accepts a slice of exactly 64,000 bytes. `enter_graphics` programs mode 13h and the palette below. It does not clear the buffer. `enter_text` programs VGA text mode 3 (80×25) and does not clear the text cells. `blit` copies the software buffer to `0xA0000`.

The proof also reads the backend back through `hw_byte`, `hardware_sum`, `dac`, `gc`, `crtc`, and `text_cell_roundtrip`. Those stay in `fb.rs`.

## Palette

Each index `i` is stored in the VGA DAC as 6-bit channels:

- red = `(i >> 5) * 9`
- green = `((i >> 2) & 7) * 9`
- blue = `(i & 3) * 21`

Index 0 is `0,0,0`. Index 1 is `0,0,21`. Index 255 is `63,63,63`. The proof reads those three entries back.

## Mode checks

After `enter_graphics`, graphics-controller register 6 is `0x05` and CRTC register 9 is `0x41`. After `enter_text`, they are `0x0E` and `0x4F`. The text-mode check then writes `0x0A41` at `0xB8000` and reads it back. Register tables are the mode 13h and mode 3 sets in `fb.rs` (misc, sequencer, CRTC, graphics, attribute).

## Optional `r3_pixels`

A module may export `r3_pixels`, `() -> i32`, the offset of a 64,000-byte buffer in its own linear memory. `apps::blit_exported` calls it, copies those bytes into the software buffer, enters mode 13h, and blits. An out-of-range offset is `memory`. A module that does not export it returns `Ok(false)` and the text path is unchanged. `check_abi` does not require the export. Integers only. Modules are still baked into the image. There are no imports.

`hello` and `spin` do not export it. Their screens stay the text screens they already had. The shell calls `blit_exported` after each text paint. On the way out of an app it restores text mode only if graphics mode was entered, then clears the console as before.

The proof's guest fixture is not a shell app. Its WAT is on `PIXELS` in `kernel/src/fbtest.rs`. Byte `i` is `i % 251`. The host blit of that buffer is part of `fb-core`. Loading `hello` in the same proof must report no export and must leave the software buffer alone.

## `fb-core`

Status 219, port byte `0x6D`, `qemu_exit::fb_core_ok`. QEMU is `-m 64M -smp 1 -vga std`. A failed check prints `R3ATH3_FB_CORE fail step=<n> detail=<text>` and exits 35. The serial log is:

```
R3ATH3_STAGE rust_main
R3ATH3_FB_CORE start
R3ATH3_FB_CORE step=1 mode=13h width=320 height=200 stride=320 format=indexed8 gc6=05 crtc9=41
R3ATH3_FB_CORE step=2 direct sum=8160000 px0=1 px319=190 px63999=254
R3ATH3_FB_CORE step=3 dac0=0,0,0 dac1=0,0,21 dac255=63,63,63
R3ATH3_FB_CORE step=4 guest sum=7999385 px0=0 px251=0 px63999=245 absent=ok
R3ATH3_FB_CORE step=5 text gc6=0e crtc9=4f cell=0a41
R3ATH3_FB_CORE step=6 heap=same frames=same
R3ATH3_FB_CORE ok
```

Step 2 fills the software buffer with `(i * 3 + 1) as u8` and blits. The sum and the three samples are read from `0xA0000`. Step 4 is the guest fixture. `absent=ok` is `hello`, which has no `r3_pixels`. Step 5 is text mode restored. Step 6 is `heap::used` and the free-frame count unchanged. The direct sum is 8160000. The guest sum is 7999385.

## Frames

The software buffer is 64,000 bytes. The link rounds the kernel image up to whole frames, so the shell image loses 16 frames. Milestone 11's shell reported `frames_total=15765`. This shell reports `frames_total=15749`. `shell-mem` requires that figure to sit in 15741..=15757. `heap_used` is still at most 4096. Builds that do not include `fb` do not carry the buffer.

## Unsafe

`rg -n 'unsafe|asm!' kernel/src` is 141 lines. Milestone 11 ended at 122. The 19 new lines are only in `kernel/src/fb.rs` (11) and `kernel/src/speaker.rs` (8). `fbtest.rs` and `speakertest.rs` have none.

## Not in this part

Glue Duty in pixels, a mouse, a graphical shell, a Bochs/VBE framebuffer, and any resolution above 320×200. Those are later.
