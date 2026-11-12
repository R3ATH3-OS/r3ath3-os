# Storage (part A: RAM disk)

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the `store` feature in this image. If the code and this document disagree, the change is not done.

There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0, target `x86_64-unknown-none`.

Part A is a RAM disk. A reboot erases it. Nothing in this image survives a reboot. NVMe, a disk image, and a copy that is still there after QEMU starts again are not in this image.

## 1. Words

- **Block:** 512 bytes, addressed by an LBA from 0.
- **Device:** something that implements `BlockDev`. This image has one: the RAM disk.
- **Store:** the `R3ATH3ST` version 1 format on that device.
- **Boot record:** one boot's copy of the flight recorder on the store. Records are numbered from 1.
- **Saved text:** a short named text the human keeps with `save`.
- **Survives a reboot:** still readable after QEMU exits and is started again. The RAM disk does not. Its bytes live in guest RAM and come back as zeros.

## 2. Block device and the RAM disk

```rust
pub const BLOCK: usize = 512;

pub enum IoError {
    Range,           // lba >= blocks()
    Device(u8, u8),  // (status code type, status code); unused by the RAM disk
    Timeout,
}

pub trait BlockDev {
    fn blocks(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError>;
    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}
```

`read` and `write` check `lba < blocks()` first and return `Range` without touching the device. On the RAM disk, `flush` returns `Ok` at once.

```rust
pub const RAM_BLOCKS: usize = 512; // 512 × 512 = 262_144 bytes
static DISK: IrqCell<[[u8; BLOCK]; RAM_BLOCKS]> = IrqCell::new([[0; BLOCK]; RAM_BLOCKS]);
pub struct RamDisk; // zero-sized handle
```

`DISK` is module-private, in `.bss`, behind the existing `IrqCell`. The initializer is all zero. The RAM disk adds no `unsafe`. It sits inside the kernel image, so the frame allocator does not hand those pages out. `read` and `write` copy one block inside `DISK.with`. That is the only work done with IF clear. `wipe_for_test` fills the disk with zeros and is compiled only for `store-core`.

Why not frames: frames are reached through raw pointers, which would be `unsafe` outside a driver. Why not the heap: `shell-mem` requires `heap_used <= 4096`, and the shell path allocates nothing.

512 blocks is 15 boot records (`store-core` step 8 fills all 15 and proves the next mount opens none) plus the 16 saved slots. The smallest device the format accepts is 41 blocks.

`mem` on the shell build of main printed `frames_total=16015`. The same command on the part A shell printed `frames_total=15945`. The drop is 70 frames. The RAM disk is 64 frames (256 KiB). The rest is code and data. A drop of more than 80 frames would have stopped the change. `shell-mem` checks `frames_free == frames_total` and a bound, not an exact total.

`size_of::<store::Mounted>()` is 40, locked by a compile-time assert.

## 3. Format `R3ATH3ST` version 1

All integers are little-endian. A check is FNV-1a 64 from the basis `0xcbf29ce484222325`, prime `0x100000001b3`, over the stated bytes. That is the flight recorder's link hash without its framing (`docs/RECORDER.md`).

| LBA | Blocks | Content |
| --- | --- | --- |
| 0 | 1 | Superblock |
| 1–4 | 4 | Saved text: 16 slots of 128 bytes |
| 5–7 | 3 | Reserved, zero when a blank disk is formatted |
| 8 + 33·(k−1) | 33 per record | Boot record `k`, for `k = 1 … slots`: one header block, then 32 entry blocks (4 × 128 bytes, 128 entries) |

`slots = min((blocks − 8) / 33, 64)`. The RAM disk has 512 blocks, so 15 slots, and 503 blocks are part of the layout.

### 3.1 Superblock (LBA 0)

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 8 | magic | `R3ATH3ST` |
| 8 | 4 | version | 1 |
| 12 | 4 | block_size | 512 |
| 16 | 8 | blocks | device blocks at format time |
| 24 | 4 | saved_lba | 1 |
| 28 | 4 | saved_blocks | 4 |
| 32 | 4 | boot_lba | 8 |
| 36 | 4 | boot_blocks | 33 |
| 40 | 4 | slots | `min((blocks − 8) / 33, 64)` |
| 44 | 4 | written as zero | 0 |
| 48 | 8 | check | check of bytes 0..48 |
| 56 | 456 | written as zero | 0 |

The check covers bytes 0..48, including the word at offset 44. Mount accepts a superblock when the magic and that check match, version is 1, and the layout fields match what this section gives for the superblock's own `blocks`. It does not require the word at offset 44 to be zero beyond what the check already covers, and it does not reject non-zero bytes at 56..512.

### 3.2 Saved-text slot

Slot `i` (0…15) is at LBA `1 + i/4`, offset `(i % 4)·128`.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | state: 0 empty, 1 used |
| 1 | 1 | name_len, 1–12 |
| 2 | 12 | name, `[a-z0-9-]`, zero padded |
| 14 | 1 | text_len, 1–58 |
| 15 | 58 | text, printable ASCII `0x20`–`0x7E`, zero padded |
| 73 | 47 | zero |
| 120 | 8 | check of bytes 0..120 |

An all-zero slot is empty. Anything else that breaks these rules is a bad slot. A bad slot is never reused. `show` lists it as `saved slot <i> bad` with `i` from 1. The saved count does not include bad slots.

`save ` (5) plus a 12-byte name, one space, and 58 bytes of text is `MAX_LINE` (76).

### 3.3 Boot-record header

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 8 | magic `R3BOOT01` |
| 8 | 4 | boot: this record's number `k` |
| 12 | 4 | lost: last saved value of the live recorder's `lost` |
| 16 | 8 | prev_head: head of record `k−1` when this record was opened (`BASIS` for `k = 1`) |
| 24 | 4 | prev_entries: entries of record `k−1` when this record was opened (0 for `k = 1`) |
| 28 | 4 | zero |
| 32 | 8 | check of bytes 0..32 |
| 40 | 472 | zero |

An all-zero header block is free (`BootScan.free = true`). A non-zero header that fails the magic, the `boot == k` check, a zero field, the padding at 40..512, or the check is a bad header: `header_ok = false`, `free = false`. That slot counts as used and is never rewritten.

### 3.4 Entry encoding

Entry `n` of record `k` is in block `8 + 33·(k−1) + 1 + (n−1)/4`, at offset `((n−1) % 4)·128`. The layout is these bytes, not Rust's `repr` of `Entry`.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | n |
| 4 | 4 | zero |
| 8 | 8 | tick |
| 16 | 8 | link |
| 24 | 1 | tag_len, 1–24 |
| 25 | 24 | tag, zero padded |
| 49 | 1 | body_len, 0–76 |
| 50 | 76 | body, zero padded |
| 126 | 2 | zero |

An empty slot is 128 zero bytes (`n = 0`). `decode_entry` accepts a slot only when `n ≥ 1`, the lengths are in range, tag and body bytes are printable ASCII, and the padding and zero fields are zero. It builds the `Entry` with `Entry::from_parts`, which checks `n` and the text and does not check the link. `scan_boot` recomputes the chain.

## 4. Mount and write-through

```rust
pub enum MountError { Io, Small, Superblock, Version, Layout, Size, NotBlank }
// words: io, small, superblock, version, layout, size, not-blank

pub enum Device { Ram, Nvme } // word(): "ram", "nvme"

pub struct Mounted {
    pub blocks: u64,
    pub slots: u32,
    pub used: u32,
    pub this: Option<u32>,
    pub formatted: bool,
    pub saved_lost: u32,
    pub saved_upto: u32,
    pub failed: bool,
    pub device: Device,
}
```

`Mounted::UNMOUNTED` has `blocks = 0`. `is_mounted` is `blocks >= 41`. A failed RAM mount leaves `STORE` at `UNMOUNTED`. `size_of::<Mounted>()` is 40, and a compile-time assert locks that. `Device::Nvme` is in the enum. This image only constructs `Device::Ram`. The `Nvme` arm of `store` prints nothing, so this image does not carry a sentence that says a boot was saved. There is no third output line.

Mount order:

1. `blocks < 41` → `Small`.
2. Read LBA 0. An I/O error is `Io`.
3. If LBA 0 is all zero, read LBA 1–7 and every slot header that fits. Any non-zero byte is `NotBlank`, and nothing is written. Otherwise write the superblock, flush, and set `formatted`.
4. Otherwise: magic or check wrong → `Superblock` (this is decided before version). `version != 1` → `Version`. `block_size`, `saved_lba`, `saved_blocks`, `boot_lba`, `boot_blocks`, or `slots` differ from section 3 for the superblock's own `blocks` → `Layout`. Superblock `blocks` below 41, or `slots` of 0, → `Small` (a store with no boot record is refused, even when the device itself is larger). Superblock `blocks` greater than the device → `Size`.
5. Count non-free headers in `used`. The first free slot `k` is `this`. If none is free, `this = None`.
6. Open `k`: previous `(entries, head)` from `scan_boot(k−1)` when that header is ok, otherwise `(0, BASIS)` when `k = 1` or the previous header is bad. Write zeros over the 32 entry blocks, write the header with `lost = 0`, flush once, then `used += 1`.

`read_superblock` is steps 1, 2, and 4 only. It does not format and does not write. It returns `(superblock.blocks, slots)`.

The shell copies `Mounted` out of `STORE`, does I/O, and writes it back. I/O never runs inside `LOG.with` or `STORE.with`. The shell is the only store task.

After an append that returned `Ok(n)`, the shell writes the block of entries `4q+1 … 4q+4` (`q = (n−1)/4`). Slots above `len` are zero. There is no read of that block. Then flush, and `saved_upto = n`. `n` above 128 returns `Range`, sets `failed`, and does not write: that entry would land on the next record's header. The shell's `LOG_CAP` is 128, and a compile-time assert keeps it at most 128, so the shell does not reach that return. After `lost` changes, the shell reads the header, rewrites `lost`, recomputes the check over bytes 0..32, writes, flushes, and sets `saved_lost`. Nothing is written when `this` is `None` or `failed` is set. `persist_entry` and `persist_lost` return `Ok` in that case, so the shell prints the failure line only on the transition.

On a write or flush error: set `failed`, stop further record writes, and print once on both channels, using the `saved_upto` from before the failed write:

```
store: write failed after entry #<saved_upto>; the rest of this boot is not saved
```

The live record and the flight-recorder rules are unchanged. The shell does not refuse agents because a record write failed. `save` and `forget` do not set `Mounted.failed`. They report their own I/O errors.

### 4.1 Reading a boot record

```rust
pub struct BootScan {
    pub header_ok: bool,
    pub free: bool,
    pub lost: u32,
    pub prev_head: u64,
    pub prev_entries: u32,
    pub entries: u32,
    pub head: u64,
    pub broken_at: Option<u32>,
}
```

`free` distinguishes an all-zero header from a bad header. A bad header returns `header_ok = false`, `free = false`, and does not read entries. A free header returns `free = true` and does not read entries.

Entries are read one block at a time. An all-zero slot ends the valid prefix, and the scan continues through entry 128 so a later non-zero slot sets `broken_at` to the first zero's number. A slot that does not decode, has `n != i`, or has a link that is not `link(prev, …)` from `BASIS` sets `broken_at = i` and stops. `entries` and `head` are the valid prefix (`head` is `BASIS` when there are none). `walk_boot` calls its callback for each valid entry before the break.

For `k = 1`, `prev` is `-`. For `k ≥ 2` with a good header, `prev` is `bad` when record `k−1`'s header is not ok, even if the stored pair is `(0, BASIS)`. Otherwise it is `ok` when `(prev_entries, prev_head)` equals `scan_boot(k−1)`'s `(entries, head)`, and `bad` otherwise.

### 4.2 What a reboot does

Every boot starts with a zeroed RAM disk, formats it, and opens boot record 1. The shell prints nothing on that success. `boots` and `boots 1` read this boot's record back from the RAM disk. That proves the write path. It does not prove that anything is still there after a reboot, because nothing is.

Only a blank disk is formatted. There is no erase command. Earlier records on a device that still holds them are read-only history. On the RAM disk the device does not still hold them: the next boot's disk is zeros.

## 5. Shell commands

The five commands are human commands. They are recorded by the existing human-line hook, with the line as typed. No agent verb reads or writes the store. Badges are not stored.

The shell mounts the RAM disk after the banner and before the first prompt. Success prints nothing, so every pre-existing shell transcript is unchanged. If that mount fails (no case reaches it), the shell prints `store: ram disk not usable (<word>)`, `STORE` stays unmounted, and the five commands print `error: store read failed` after the checks below that do not need the device.

Each command sets `last=ok` on success and `last=cmd` on any `error:` line.

| Command | Prints |
| --- | --- |
| `store` | Two lines |
| `boots` | One line per non-free slot, then a summary |
| `boots K` | The record's entries, then a result line |
| `save NAME TEXT` | `saved <NAME> to ram` |
| `show` | `saved <NAME>` per used slot, `saved slot <i> bad` per bad slot, then `saved <u> of 16 on ram` |
| `show NAME` | `<NAME> = <TEXT>` |
| `forget NAME` | `forgot <NAME>` |

`save` overwrites a used slot with the same name, otherwise the first empty slot. `save` and `forget` are one block read-modify-write plus a flush, before the success line. They still run when `this` is `None` and after record writes have failed.

Checks, first failure wins:

| Check | Error line |
| --- | --- |
| `store` with any argument | `error: usage: store` |
| `boots` with two or more words | `error: usage: boots [K]` |
| `save` without a name or without text | `error: usage: save NAME TEXT` |
| name not 1–12 bytes of `[a-z0-9-]` | `error: bad saved name: <NAME>` |
| text longer than 58 bytes | `error: saved text too long (max 58)` |
| `show` with two or more words | `error: usage: show [NAME]` |
| `forget` with no word or two or more | `error: usage: forget NAME` |
| the store is not mounted, or a read fails | `error: store read failed` |
| `boots K`: not a number, 0, above `slots`, or a free slot | `error: no boot <word as typed>` |
| `show NAME` or `forget NAME`: no used slot has that name, including a name that could never be saved | `error: no saved name <NAME>` |
| a new name and no empty slot | `error: saved data full (16 names)` |
| a write or flush fails during `save` or `forget` | `error: store write failed` |

`boots` with one word loads the store before it parses the word. `boots x` on a mounted disk is `error: no boot x`. The usage, name, and length checks in the rows above that one run before the mount check. `save` keeps inner spaces in the text. The line is already trimmed.

`store` line 1:

```
store device=ram blocks=512 boots=<used> of <slots> saved=<u> of 16
```

`saved` is the count of used slots. Line 2, for this image:

```
store: write failed after entry #<n>; the rest of this boot is not saved
store: ram disk, erased by a reboot; this boot is boot <k>
store: ram disk full (<slots> boots); this boot is not saved
```

The first applies when `failed` is set. The second applies when this boot has a record. The third applies when no slot was free. There is no third output line in this image.

`boots` prints, for each non-free slot, one of:

```
boot <k> entries=<e> lost=<l> head=<16 hex> chain=<ok|#j> prev=<ok|bad|->
boot <k> bad header
```

then:

```
boots <used> of <slots> used; this is boot <k>
boots <used> of <slots> used; this boot is not saved
```

`chain=#j` means the record's own chain breaks at entry `j`. `entries` and `head` are the valid prefix. `prev` is the cross-boot check. `boots K` prints each valid entry in the flight recorder's two-line format, then one of:

```
boot <K> ok entries=<e> lost=<l> head=<16 hex>
boot <K>: chain broken at #<j>
boot <K>: bad header
```

The ok line sets `last=ok`. The other two set `last=cmd`. A bad header prints no entries. `boots K` does not print `prev`.

`help` is 24 lines. These five sit after `selftest` and before `run`:

```
store       storage device and what survives a reboot
boots [K]   list saved boot records, or print boot K
save N TEXT keep TEXT under name N
show [N]    list saved names, or the text saved as N
forget N    remove the text saved as N
```

`R3GATE ASK ai1 2 save x y` is a bad line with `reason=verb`, the same as any unknown verb.

## 6. Integrity

| Structure | Check | Detects |
| --- | --- | --- |
| Superblock | FNV over bytes 0..48 | a bad magic or check, refused as `superblock` |
| Saved slot | FNV over bytes 0..120 | a damaged slot, listed as bad |
| Boot header | FNV over bytes 0..32, `boot == k`, padding zero | a damaged or moved header, listed as bad |
| Entries | the flight-recorder link chain, recomputed from `BASIS` | a changed, removed, inserted, or reordered entry |
| Across boots | `prev_entries` / `prev_head` in record `k` | an earlier record changed after record `k` was opened |

What this does not detect:

- A host that rewrites a record and recomputes its chain, its header check, and the next record's `prev`. FNV-1a is not a secret and not collision-resistant. SHA-256 was considered for this milestone and not adopted.
- A host that zeroes the newest headers. The records that remain can still look consistent.
- An edit made before the next record was opened. Then `prev` describes the edited record, so it shows `ok`, while that record's own chain still shows the break.

A human who writes down `head=` from `boots` or from `log` can later tell whether that record changed. That is the check's real use. The kernel does not treat an earlier boot record as a permission.

## 7. `selftest`

`selftest` runs eight checks: `timer`, `sched`, `frames`, `script`, `gate`, `recorder`, `store`, `heap`. The shell calls `store::check_live` before `selftest::run` and passes the result in. The check only reads:

| Order | Condition | Detail |
| --- | --- | --- |
| 1 | `read_superblock` fails | `superblock unreadable` |
| 2 | `failed` is set | `store write failed` |
| 3 | `this = Some(k)` and the scan errors, the header is bad, the chain breaks, `entries != live_len`, or `head != live_head` | `record copy differs` |
| 4 | otherwise | ok |

When `this` is `None`, check 3 is skipped and the check passes. `selftest` does not write, format, or flush. The card is:

```
selftest: 8 checks
selftest timer    ok
selftest sched    ok
selftest frames   ok
selftest script   ok
selftest gate     ok
selftest recorder ok
selftest store    ok
selftest heap     ok
selftest: 8 of 8 ok
```

The summary entry's body is `ok=8 fail=0`. `recorder-core` step 10 prints `selftest: 7 of 8 ok, 1 failed`.

## 8. `store-core`

`storetest::run_core` runs on the RAM disk with no shell. `Limit { blocks }` reports fewer blocks than 512 and returns `Range` above that. `FailWrites` counts `write` calls while armed and returns `Timeout`; `flush` is not counted; `Range` is checked first. The six entries are the `recorder-core` six (ticks 10–60), including body `ok=7 fail=0` on entry 6. Links: #1 `ebb95e48118d0282`, #2 `4d9b4cc92b89320a`, #6 `68712f5597559a1a`.

Step 5 flips bit 0 of LBA 9 byte 306 (entry 3's first body byte, `g` → `f`) and expects `entries=2`, `head=4d9b4cc92b89320a`, `broken=3`, and boot 2 `prev=bad`, then restores the byte. Step 6 flips LBA 0 byte 8 (the check fails before version is read) and then sets version 2 with a recomputed check. Step 7: `read_superblock` on a device that reports 256 blocks is `size`; a 40-block device is `small`; a zero disk with LBA 5 byte 0 set is `not-blank` and LBA 0 stays zero. Step 8 opens slots 1 through 15; the 16th mount has `this = None` and `used = 15`. Step 9's `writes_after` is 0: the failed write was the only attempt after the flag was armed. An internal mismatch panics (status 35). Success prints this log and exits 247 (`0x7B`):

```
R3ATH3_STAGE rust_main
R3ATH3_STORE_CORE step=1 blank formatted=yes blocks=512 slots=15 this=1
R3ATH3_STORE_CORE step=2 persisted=6 boot1 entries=6 lost=1 head=68712f5597559a1a chain=ok
R3ATH3_STORE_CORE step=3 remount formatted=no this=2 used=2 prev=ok
R3ATH3_STORE_CORE step=4 saved=1 note=hello n2=gone
R3ATH3_STORE_CORE step=5 tamper boot1 entries=2 head=4d9b4cc92b89320a broken=3 boot2 prev=bad restored=ok
R3ATH3_STORE_CORE step=6 badcheck=superblock badversion=version restored=ok
R3ATH3_STORE_CORE step=7 short=size small=small notblank=not-blank lba0=zero
R3ATH3_STORE_CORE step=8 opened=15 sixteenth=none used=15
R3ATH3_STORE_CORE step=9 writefail saved_upto=1 failed=yes writes_after=0
R3ATH3_STORE_CORE step=10 encode=128 decode=1,10,human last=ok,help link=ebb95e48118d0282
R3ATH3_STORE_CORE ok steps=10
```

## 9. Smoke

`./scripts/smoke.sh` runs 64 QEMU cases, plus `harness-selftest`, and 27 build lines. The new build is `store-core`. No pre-existing case gains a disk image or a new QEMU flag.

| Case | Runner | Status | Judge | Limit |
| --- | --- | --- | --- | --- |
| `store-core` | `expect_case` | 247 | exact | 12 s |
| `store-ram` | `expect_gate` | 209 | `store:s1` | 20 s |
| `store-usage` | `expect_shell` | 211 | exact | 40 s |

`store-ram` types `store`, `save note hello there`, `show`, `show note`, `grant ai1 say`, then `R3GATE ASK ai1 1 say hi` and `R3GATE ASK ai1 2 save x y`, then `forget note`, `show`, `boots`, `boots 1`, `replay`, `exit`. `boots` lists `entries=10` because it runs after entry 10, and that head is the link of #10. `boots 1` prints #1–#11. `replay` prints #1–#12. The alias `s1=live` requires those two groups to agree on every entry number both printed.

| # | Tag | Body |
| --- | --- | --- |
| 1 | human last=ok | store |
| 2 | human last=ok | save note hello there |
| 3 | human last=ok | show |
| 4 | human last=ok | show note |
| 5 | gate | grant ai1 say |
| 6 | human last=ok | grant ai1 say |
| 7 | agent ai1 | say seq=1 done text=hi |
| 8 | agent ai1 | - seq=2 bad reason=verb |
| 9 | human last=ok | forget note |
| 10 | human last=ok | show |
| 11 | human last=ok | boots |
| 12 | human last=ok | boots 1 |

`store-usage` types 32 lines and `exit`, and exits 211. It covers the usage, name, length, missing-name, and full-table errors in section 5, fills all 16 names (`k1`…`k16`), overwrites `k1`, and refuses `k17`. The line `save a` plus 69 `x` is 76 bytes, so the shell accepts it and the 58-byte text rule refuses it. It does not cover `error: store read failed` or `error: store write failed`. The RAM disk does not fail a read or a write, so those two lines are not reached in this image. They are reserved for a later device that can fail a read or a write.

The `store` judge masks `t=<digits>`, `link=<16 hex>`, and `head=<16 hex>` only on header lines and on summary lines (`log`, `replay`, a `boot K` list line, and a `boot K ok` line), the same way the `log` judge masks header and summary lines. Every other line is compared as written. It then checks the values. Entries after `r3> boots K` are group `sK`. Entries after `r3> log` or `r3> replay` are group `live`. Each group is one chain from `BASIS` when it starts at #1; a group that starts later anchors its first link and checks the rest, and ticks do not decrease. A list line `boot K entries=e … head=X` must use `BASIS` when `e = 0`, and must use entry `e`'s link when group `sK` printed that entry. `boot K ok … head` and `replay ok … head` equal the last link of that group. `store:s1` also requires group `s1` to match group `live` on every common entry number. The `ref` argument is accepted and unused by the cases in this image.

Intentional changes to older expected transcripts: `shell-basic` help is 24 lines, `shell-selftest` prints the eight-check card, and `recorder-core` step 10 says `selftest: 7 of 8 ok, 1 failed`. 209 covers 14 cases, 211 covers 8, and 247 covers `store-core`.

The harness self-test rejects a `store-core` log at status 245, with step 4 removed, with steps 4 and 5 swapped, and with one hex digit of step 2's head changed. It rejects a filled `store-ram` log with one link digit changed in the `boots 1` block, one body byte changed in the `replay` block, `s1` refilled with different ticks (each chain still valid, so only the alias fails), the `boots` list head replaced by #9's link, #5 removed from the `boots 1` block, and status 211. `store-usage` does not compare equal to `log-usage` or `shell-usage` in either direction. The filled `store-ram` log does not satisfy `log-record`'s expected text.

## 10. Not in this image

NVMe, PCI, port `inl`, an MMIO window, `nvme-core`, a disk image on any QEMU command line, a copy that survives a reboot, a file system, a VFS, an erase command, an agent storage verb, and SHA-256. `0x7C` (status 249) is unused and is the value set aside for a later NVMe proof. The other unused odd statuses are 203, 205, 207, 219, 221, 223, 237, 239, 251, 253, and 255.
