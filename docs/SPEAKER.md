# PC speaker

Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".

Part A of milestone 12 is on main: square-wave tones on the PC speaker, PIT channel 2 only. Part B plays Glue Duty on that speaker. There is no AC97, no HD Audio, and no PCM. The proof does not capture audio.

## Driver

`kernel/src/speaker.rs`. Callers use `tone` and `silence`. Every `unsafe` block has a `// SAFETY:` comment.

| Port | Use |
| --- | --- |
| `0x42` | Channel 2 data |
| `0x43` | Command, channel-2 bytes only |
| `0x61` | Speaker gate. Only bits 0 and 1 change |

Command bytes:

- `0xB6` programs channel 2, low byte then high byte, mode 3 (square wave), binary.
- `0x80` latches the channel 2 count.

Both have channel bits `10`. The driver never writes port `0x40` and never writes a channel-0 command. Channel 0 stays the 100 Hz timer: command `0x34`, divisor 11932, ports `0x40` and `0x43` as `irq.rs` programmed them. Ports `0x60` and `0x64` are not used.

`tone(0)` and `silence` close the gate and do not reprogram channel 2. Any other `hz` computes

```text
divisor = 1_193_182 / hz
```

and saturates that to `1..=65535`. The gate is forced off, the divisor is written, the count is latched, then bits 0 and 1 of port `0x61` are set. The write is `(inb(0x61) & 0xFC) | bits`, so every other bit is the bit just read.

`500` Hz is divisor 2386. `2000` Hz is divisor 596. Both divide the PIT clock in integers and both divisors are even.

## Latch

Mode 3 counts down by two per PIT clock, so a latched count stays even when the divisor is even. The latch is read in the same port-I/O window as the reload write. Host time can move that count. `speaker-core` accepts an even value in `divisor - 20 .. divisor` (greater than or equal to `divisor - 20`, and less than `divisor`). The success line says `latched=ok`. The raw count is printed only on the fail line. A different divisor still cannot produce a passing log, because the log records the divisor and the window is checked against that divisor.

## Timer

While the 500 Hz tone is gated on, `irq::wait_ticks(5)` must return 5. That is the 100 Hz channel 0 still advancing. The speaker proof does not read port `0x40`.

## `speaker-core`

Status 221, port byte `0x6E`, `qemu_exit::speaker_core_ok`. QEMU is `-m 64M -smp 1` and `-vga none`, the same as the other non-graphics proofs. A failed check prints `R3ATH3_SPEAKER_CORE fail ...` and exits 35. The serial log is:

```
R3ATH3_STAGE rust_main
R3ATH3_SPEAKER_CORE start
R3ATH3_SPEAKER_CORE step=1 off hz=0 gate=0
R3ATH3_SPEAKER_CORE step=2 hz=500 divisor=2386 latched=ok gate=3
R3ATH3_SPEAKER_CORE step=3 ticks=5
R3ATH3_SPEAKER_CORE step=4 hz=2000 divisor=596 latched=ok gate=3
R3ATH3_SPEAKER_CORE step=5 off hz=0 gate=0
R3ATH3_SPEAKER_CORE step=6 heap=same frames=same
R3ATH3_SPEAKER_CORE ok
```

The latch and the reload write run inside `irq::without_interrupts`, so the timer handler cannot split them. Channel 2's command bytes are the only writes to port `0x43` from this driver. `gate` is port `0x61` bits 0 and 1 after the update. Step 6 is `heap::used` and the free-frame count unchanged.

`speaker` is internal. `shell`, `speaker-core`, and `glue-tone` turn it on. The shell calls `silence` before it prints `R3ATH3_SHELL_READY`, which closes the gate and does not change the transcript. `speaker-core` is a test mode, in the at-most-one assert and in both `boot_ok` exclusion lists.

## Glue Duty tones

A module may export `r3_tone`, `() -> i32`, the frequency word. Zero is silence. `check_abi` does not require it. There is still no import. On the shell, `play_exported` calls `speaker::tone` only when that word changes, including the change to 0, and returns that `Played` value. A repeat of the same word returns `None` and does not touch the PIT. `finish` calls `silence` and clears that cache before it restores text mode, so the next app programs from a fresh word. The cache and that silence block are both behind the `speaker` feature. The shell does not print the latch, so `app-glue`'s transcript does not gain a tone line. `hello` and `spin` do not export `r3_tone`.

The word is a presentation of the same rules. An event line wins over the state drone:

| Condition | Hz | Divisor |
| --- | --- | --- |
| log `Scrubbed a blob.` | 500 | 2386 |
| log `Solvent blast.` | 640 | 1864 |
| log `A blob reached the discharge.` | 180 | 6628 |
| log `Shift complete.` | 2000 | 596 |
| log `The belt gummed up.` | 80 | 14914 |
| title | 250 | 4772 |
| playing | 400 | 2982 |
| paused | 0 |  |
| shift done | 2000 | 596 |
| game over | 80 | 14914 |

Every non-zero divisor is even. On the proof script the first scrub is step 34.

`glue-tone` is the proof. Status 205, port byte `0x66`, `qemu_exit::glue_tone_ok`. QEMU is `-m 64M -smp 1` and `-vga none`. It calls `play_exported` and `finish`, then reads port `0x61` and latches channel 2 with `speaker::sample`. That read does not program a divisor and does not change the gate.

The programming check is the latch `speaker::tone` already takes in the same port-I/O window `speaker-core` proves: even, and in `divisor - 20 .. divisor`. This proof also accepts the reload value itself (`latched == divisor`). Reprogramming a running channel 2 on this QEMU can return that value before the first PIT clock. `speaker-core`'s check stays half-open. The success line says `latched=ok`. The raw count is printed only on the fail line.

The read after `play_exported` returns is a later count. It is not a second 20-count window. For a sounding word it must be even, nonzero, and at most that divisor, with gate bits `3`. For silence the gate bits are `0`.

```
R3ATH3_STAGE rust_main
R3ATH3_GLUE_TONE start
R3ATH3_GLUE_TONE step=1 hz=250 divisor=4772 latched=ok gate=3
R3ATH3_GLUE_TONE step=2 hz=400 divisor=2982 latched=ok gate=3
R3ATH3_GLUE_TONE step=3 ticks=5
R3ATH3_GLUE_TONE step=4 hz=0 gate=0
R3ATH3_GLUE_TONE step=5 hz=400 divisor=2982 latched=ok gate=3
R3ATH3_GLUE_TONE step=6 scrub=34 hz=500 divisor=2386 latched=ok gate=3
R3ATH3_GLUE_TONE step=7 same hz=500 changed=no gate=3
app glue-duty: quit
R3ATH3_GLUE_TONE step=8 finish gate=0
R3ATH3_GLUE_TONE step=9 again hz=500 divisor=2386 latched=ok gate=3
R3ATH3_GLUE_TONE step=10 off hz=0 gate=0
R3ATH3_GLUE_TONE step=11 absent=ok
R3ATH3_GLUE_TONE step=12 heap=same frames=same
R3ATH3_GLUE_TONE ok
```

Steps 1, 2, 4, 5, 6, and 9 go through `play_exported`, which programs only when the word changed. Step 3 is `irq::wait_ticks(5)` while the 400 Hz tone is gated on. That is channel 0 still advancing. Step 4 is the change to 0, which closes the gate and does not reprogram channel 2. Step 6 is a fresh instance stepped to the first scrub. Step 7 plays that same 500 Hz word again: `play_exported` returns `None`, and the gate is still `3`. `finish` then prints `app glue-duty: quit`, calls `silence`, and clears the cache. Step 8 reads the gate as `0`. Step 9 plays 500 Hz again, which programs because the cache was cleared. Step 10 is `tone(0)` and another gate read of `0`. Step 11 is `hello`, which has no `r3_tone`. Step 12 is `heap::used` and the free-frame count unchanged. Channel 0 stays divisor 11932 on port `0x40`. Ports `0x60` and `0x64` are not used.

## Not in this part

Sampled audio, and any device other than the PC speaker. The shell has no new verb for sound. Help stays 24 lines.
