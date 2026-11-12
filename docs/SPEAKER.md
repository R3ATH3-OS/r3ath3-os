# PC speaker

Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".

Part A of milestone 12. Square-wave tones on the PC speaker, PIT channel 2 only. There is no AC97, no HD Audio, and no PCM. The proof does not capture audio. Glue Duty tones are part B.

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

`speaker` is internal. `shell` and `speaker-core` turn it on. The shell calls `silence` before it prints `R3ATH3_SHELL_READY`, which closes the gate and does not change the transcript. `speaker-core` is a test mode, in the at-most-one assert and in both `boot_ok` exclusion lists.

## Not in this part

Glue Duty tones, sampled audio, and any device other than the PC speaker. The shell has no new verb for sound. Help stays 24 lines.
