# Framebuffer

Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".

Part A of milestone 12 is on main: VGA mode 13h, 320×200×8, and the path back to text mode 3. Part B draws Glue Duty in that buffer. The shell stays an 80×25 text shell. Graphics mode is only while a pixel path runs. Higher resolution is later, not this milestone. There is no mouse and no graphical shell.

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
pub fn with_bytes<R>(f: impl FnOnce(&mut [u8]) -> R) -> Option<R>;
pub fn enter_graphics();
pub fn enter_text();
pub fn in_graphics() -> bool;
pub fn copy_from(src: &[u8]) -> bool;
pub fn blit() -> bool;
```

`info` is the view milestone 13 can retarget. `base` points at the software buffer. For this backend, `width` is 320, `height` is 200, `stride` is 320, and `format` is `Indexed8`. `with_bytes` is the only borrow of that buffer. It returns `None` when a borrow is already in progress, the same shape as the WebAssembly arena's `with_instance`. `copy_from` accepts a slice of exactly 64,000 bytes and returns `false` when the buffer is busy. `enter_graphics` programs mode 13h and the palette below. The first call also saves the plane-2 font and DAC entries 0–63. It does not clear the buffer. `enter_text` programs VGA text mode 3 (80×25), writes that font and those DAC entries back, and does not clear the text cells. `blit` copies the software buffer to `0xA0000` and returns `false` when the buffer is already borrowed, in which case nothing is copied. A later paint does not call `enter_graphics` again while graphics mode is already on.

The proof also reads the backend back through `hw_byte`, `hardware_sum`, `dac`, `gc`, `crtc`, and `text_cell_roundtrip`. Those stay in `fb.rs`.

## Palette

Each index `i` is stored in the VGA DAC as 6-bit channels:

- red = `(i >> 5) * 9`
- green = `((i >> 2) & 7) * 9`
- blue = `(i & 3) * 21`

Index 0 is `0,0,0`. Index 1 is `0,0,21`. Index 255 is `63,63,63`. The proof reads those three entries back.

## Mode checks

After `enter_graphics`, graphics-controller register 6 is `0x05` and CRTC register 9 is `0x41`. After `enter_text`, they are `0x0E` and `0x4F`. The text-mode check then writes `0x0A41` at `0xB8000` and reads it back. Register tables are the mode 13h and mode 3 sets in `fb.rs` (misc, sequencer, CRTC, graphics, attribute).

Chain-4 writes at `0xA0000` overwrite plane 2, which is the font the text renderer reads. The graphics palette overwrites DAC entries the text attributes use: attribute `0x0A` selects DAC `0x3A` (light green) and white selects DAC `0x3F`. On the first `enter_graphics` the driver saves the 8 KiB font (256 glyphs, 32 bytes each) and DAC entries 0–63 into `.bss`. `enter_text` writes both back. Plane-2 access sets sequencer 2 to `0x04`, sequencer 4 to `0x06`, graphics-controller 4 to `0x02`, 5 to `0x00`, and 6 to `0x05`, then restores those five registers. The proof's font checksum is FNV-1a of those 8192 bytes, `c8dbddd1` on this QEMU. A QEMU upgrade, or a SeaBIOS font change, changes that hash and the expected log must be updated with it. DAC `0x07` is `42,42,42`, `0x3A` is `21,63,21`, and `0x3F` is `63,63,63`. Step 6 repeats the check through `apps::finish`, the shell's exit path. Before `finish` it requires `hardware_sum` to equal the guest sum, so a blit that did not run cannot pass.

## Optional `r3_pixels`

A module may export `r3_pixels`, `() -> i32`, the offset of a 64,000-byte buffer in its own linear memory. `apps::blit_exported` calls it, copies those bytes into the software buffer, enters mode 13h only when graphics mode is not already on, and blits. An out-of-range offset is `memory`. A module that does not export it returns `Ok(false)` and the text path is unchanged. `check_abi` does not require the export. Integers only. Modules are still baked into the image. There are no imports.

`hello` and `spin` do not export it. Their screens stay the text screens they already had. Glue Duty does export it. The buffer lives in the guest's `Game` and is a picture of the same 80×25 grid `text.rs` already paints: each cell is a 4×8 block, a 3×5 glyph, and one byte of the cell itself so two different grid bytes cannot paint the same block. The shell calls `blit_exported` after each text paint. On the way out of an app, `finish` restores text mode, the plane-2 font, and DAC entries 0–63 when graphics mode was entered, then clears the console.

## Glue Duty pixels

`glue-pixels` is the proof. Status 203, port byte `0x65`, `qemu_exit::glue_pixels_ok`. QEMU is `-m 64M -smp 1 -vga std`. The serial log is:

```
R3ATH3_STAGE rust_main
R3ATH3_GLUE_PIXELS start
R3ATH3_GLUE_PIXELS step=1 mode=13h width=320 height=200 stride=320 format=indexed8 gc6=05 crtc9=41
R3ATH3_GLUE_PIXELS step=2 title sum=660216 frame=fbc1f073536db373 px0=0 px1=28 px63999=0 hw=match
R3ATH3_GLUE_PIXELS step=3 play sum=574839 frame=f14e49b47018956c changed=ok hw=match
R3ATH3_GLUE_PIXELS step=4 absent=ok sum=same
R3ATH3_GLUE_PIXELS step=5 text gc6=0e crtc9=4f cell=0a41 font=c8dbddd1 dac07=42,42,42 dac3a=21,63,21 dac3f=63,63,63
app glue-duty: quit
R3ATH3_GLUE_PIXELS step=6 finish gc6=0e crtc9=4f font=c8dbddd1 dac07=42,42,42 dac3a=21,63,21 dac3f=63,63,63
R3ATH3_GLUE_PIXELS step=7 heap=same frames=same
R3ATH3_GLUE_PIXELS ok
```

Step 2 is the title after `r3_init(1)`. `frame` is FNV-1a 64 of the 64,000 bytes. `hw=match` is that same buffer at `0xA0000`. Step 3 is Space and one step (state 1). Step 4 loads `hello`, which has no `r3_pixels`, and the hardware sum stays the play sum. Step 5 is `enter_text`: the font and DAC match the samples taken before graphics mode. Step 6 enters graphics again, blits, and `finish` prints `app glue-duty: quit` and leaves text mode, the font, and the DAC. Step 7 is `heap::used` and the free-frame count unchanged.

`app-glue` dumps those 64,000 bytes from `0xA0000` at the title and compares them to `apps/glue-duty/golden/app-glue.pix`. The text golden `app-glue.vga` stays a host unit test of the grid. `wasm-glue` hashes the same pixel buffer on both hosts. Spec for the game: `docs/WASM.md`.

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
R3ATH3_FB_CORE step=5 text gc6=0e crtc9=4f cell=0a41 font=c8dbddd1 dac07=42,42,42 dac3a=21,63,21 dac3f=63,63,63
app pixels: quit
R3ATH3_FB_CORE step=6 finish gc6=0e crtc9=4f font=c8dbddd1 dac07=42,42,42 dac3a=21,63,21 dac3f=63,63,63
R3ATH3_FB_CORE step=7 heap=same frames=same
R3ATH3_FB_CORE ok
```

Step 2 fills the software buffer with `(i * 3 + 1) as u8` and blits. The sum and the three samples are read from `0xA0000`. Step 4 is the guest fixture. `absent=ok` is `hello`, which has no `r3_pixels`. Step 5 is `enter_text`: the font checksum and DAC `0x07`, `0x3A`, and `0x3F` match the values sampled before graphics mode. Step 6 enters graphics again, blits, and requires the hardware sum to equal the guest sum before `apps::finish`. That sum is the chain-4 write that overwrites plane 2. `finish` prints `app pixels: quit` and must leave the same font and DAC. Step 7 is `heap::used` and the free-frame count unchanged. The direct sum is 8160000. The guest sum is 7999385.

## Frames

The software buffer is 64,000 bytes. The link rounds the kernel image up to whole frames, so the shell image loses 16 frames for that buffer. The saved font and DAC are another 8 KiB plus 192 bytes in `.bss`, which the link rounds to 3 frames. Milestone 11's shell reported `frames_total=15765`. Before the font save this shell reported `frames_total=15749`. Part A reported `frames_total=15746`. Part B's shell reports `frames_total=15747`. `shell-mem` still requires 15741..=15757. `heap_used` is still at most 4096. On this shell image `heap_used` is 0. Builds that do not include `fb` do not carry the buffer or the save.

## Unsafe

`rg -n 'unsafe|asm!' kernel/src` is 145 lines. Milestone 11 ended at 122. The framebuffer and speaker lines are only in `kernel/src/fb.rs` (21) and `kernel/src/speaker.rs` (2). `fbtest.rs` and `speakertest.rs` have none. Every block in those two files has a `// SAFETY:` comment.

## Not in this part

A mouse, a graphical shell, a Bochs/VBE framebuffer, and any resolution above 320×200. Those are later.
