# Interactive shell

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the `shell` feature. If the code and this document disagree, the change is not done.

The shell is a human keyboard interface. It adds no model or agent input, no network, and no action gate. `run` executes only the existing fixed-buffer task language.

## 1. Scope

In this image:

1. A `shell` feature build: a line-editing prompt `r3> ` that reads the PS/2 keyboard and echoes to COM1 and to the VGA text console.
2. A VGA text console: 80×25, cursor tracking, hardware cursor, scrolling, clear, backspace.
3. Built-ins: `help`, `echo`, `clear`, `uptime`, `mem`, `ps`, `spawn`, `wait`, `run`, `exit`, `halt`.
4. Backspace, a fixed 76-byte line limit, an unknown-command error, and empty lines.
5. `run` executes an inline task-language script and survives its errors. `script` and `kbd-script` output is unchanged.

Not in this image: `ls`, `cat`, or any file system; processes or background jobs; command history; tab completion; arrow keys, Home, End, Delete, or editing mid-line; Ctrl-C or other control keys; Caps Lock; pipes, redirection, shell variables, globbing, or quoting rules in the shell itself; scrollback; colors other than green on black; serial input to the shell; a mouse; a GUI or framebuffer. The default boot path is unchanged: it still prints the boot marker and exits 33. The shell build runs the kernel scheduler (`docs/SCHEDULER.md`). `spawn` starts a demo task. It is not a process, and `wait` on `spin` blocks forever because there is no Ctrl-C.

## 2. Where it plugs in

`kernel_main` order is unchanged through `mem::init()`. On the `shell` feature, `sched::init(true)` then runs, and `shell::run()` does not return. `shell` is one of the test features in the "at most one" assert and in both `boot_ok` `cfg` lists. `script` is compiled for `script`, `kbd-script`, or `shell`. The `shell` feature depends on `sched`.

COM1 is the judged channel. It carries the prompt, echoed keys, command output, and `R3ATH3_*` lines. VGA shows the same transcript, except `R3ATH3_*` lines are serial-only. Line endings are `\n` only.

## 3. VGA console

Buffer `0xB8000`, 80 columns by 25 rows. Each cell is `attribute << 8 | byte` with attribute `0x0A` (green on black). Every console access uses `read_volatile` / `write_volatile`. Cursor state is a private static. `with_console` hands it to a closure, and the `&mut` cannot escape. Only the shell task calls the console. IRQ handlers and other tasks do not touch VGA. A preemption in the middle of a console call is harmless because no other task enters the module.

- `console_clear` fills all 2000 cells with `0x0A20`, sets row and column to 0, and updates the hardware cursor.
- `console_put` treats `\n` as "column 0 of the next row". A byte in `0x20`–`0x7E` is stored and the column advances. Any other byte is stored as `?`.
- Wrap is deferred. When the column is already 80, the next printable byte first moves to column 0 of the next row. A `\n` at column 80 moves to the next row once and does not insert a blank line.
- Moving past row 24 scrolls: rows 1–24 copy to rows 0–23, row 24 fills with spaces, and the row stays 24.
- `console_backspace` clears the previous cell on the same row. It does nothing at column 0 and never moves to the previous row. The shell keeps the prompt plus the line within 80 columns, so backspace does not need to cross a row.
- After each of those calls, the hardware cursor position `row * 80 + min(column, 79)` is written to CRTC registers `0x0E` and `0x0F` through ports `0x3D4` and `0x3D5`. Smoke does not judge the cursor.

`write_marker` and `write_line` keep their previous behavior. They are not the console.

## 4. Serial-only control bytes

- A backspace that removes a byte echoes `\x08 \x08` (backspace, space, backspace) on COM1 and calls `console_backspace` on VGA.
- `clear` writes `\x1b[2J\x1b[H` on COM1 and calls `console_clear` on VGA.

## 5. Keyboard

The shell reads `irq::pop_key`. In this build it is the only caller. When the ring is empty it calls `sched::yield_now` and loops. With nothing else ready, `pick_next` returns the shell and `yield_now` returns at once, so a key is noticed within one tick (10 ms). With six `spin` tasks the shell runs once per round of quanta, at most 70 ms. When the shell is blocked in `wait` and nothing else is ready, idle calls `irq::idle_hlt`.

Scan code set 1 also maps `0x27` to `;` (Shift `:`) and `0x28` to `'` (Shift `"`). Keys `translate` does not map are ignored. The shell does not reset or print `KB_DROPS`.

## 6. Line editing

The line buffer is 76 bytes, so the 4-byte prompt plus the line fill one row. `last` starts as ok.

- A printable key is stored and echoed on both channels while the line is shorter than 76. The 77th printable key is not stored or echoed, and sets overflow.
- Backspace at a non-empty line drops one byte, echoes `\x08 \x08` on COM1, and backspaces one VGA cell. Backspace on an empty line echoes nothing. Backspace does not clear overflow.
- Enter echoes `\n` on both channels. If overflow is set, the shell prints `error: line too long (max 76)` and does not run the line. `last` becomes overflow. A line that is empty or only spaces does not run, and `last` stays as it was. Otherwise the line is split on ASCII spaces and dispatched. Leading and trailing spaces are ignored. The first word is the command, case-sensitive. Then the buffer and the overflow flag reset, and the prompt is printed again.
- `exit` and `halt` do not return to the prompt.

## 7. Built-ins

Output lines end in `\n` and go to both channels unless marked serial-only.

| Command | Arguments | Output | `last` |
| --- | --- | --- | --- |
| `help` | none | The 11 lines below | ok |
| `echo` | any | The words joined by single spaces. `echo` alone prints an empty line. Quotes are not special. | ok |
| `clear` | none | Serial `\x1b[2J\x1b[H`; VGA clear | ok |
| `uptime` | none | `uptime ticks=<n> hz=100 seconds=<s>.<cc>` with `n = irq::ticks()`, `s = n / 100`, and `cc = n % 100` as two digits | ok |
| `mem` | none | `mem heap_used=<heap::used()> heap_size=1048576 frames_total=<t> frames_free=<f>` from `frames::stats()` | ok |
| `ps` | none | One line per non-free slot, id order: `task id=<n> name=<name> state=<ready\|running\|sleeping\|blocked\|exited>` | ok |
| `spawn` | one word, `spin` or `nap` | `spawned id=<n> name=<demo>` | ok |
| `wait` | one decimal id | `exited id=<n>` after the slot is reaped | ok |
| `run` | the rest of the line after `run` and its following spaces, raw | Script lines (`text` plus `\n`) on both channels. On error: `error: script line=<n> reason=<word>` | ok, or script on error |
| `exit` | none | Serial-only `R3ATH3_SHELL_EXIT last=<ok\|cmd\|overflow\|script>`, then the status in section 8 | n/a |
| `halt` | none | Serial-only `R3ATH3_SHELL_HALT`, then status 217, whatever `last` is | n/a |

An unknown first word prints `error: unknown command: <word>` and sets `last` to cmd. Extra arguments to `help`, `clear`, `uptime`, `mem`, `ps`, `exit`, or `halt`, or `run` with nothing after it, print `error: usage: <command>` (`error: usage: run SCRIPT` for `run`, `error: usage: ps` for `ps`). `spawn` with no word or extra words prints `error: usage: spawn DEMO`. Any other demo word prints `error: unknown demo: <word>`. A full table prints `error: task table full`. `wait` with a missing or extra argument prints `error: usage: wait ID`. A non-number, a value outside 2–7, or a free slot prints `error: no task <arg>` using the raw argument. Each of those errors sets `last` to cmd. The shell does not exit. `spin` loops and never returns. `nap` sleeps 20 ticks and returns. `wait` on `spin` blocks forever.

`help` text, exactly:

```
help        list commands
echo TEXT   print TEXT
clear       clear the screen
uptime      timer ticks and seconds since boot
mem         heap and frame use
ps          list tasks
spawn DEMO  start demo task spin or nap
wait ID     wait for a task to exit
run SCRIPT  run a task script, lines separated by ;
exit        leave the shell with the last status
halt        stop the machine
```

Startup, in order: serial-only `R3ATH3_SHELL_READY`, a VGA clear with no ANSI sequence, then `R3@TH3 shell. Type help for commands.` on both channels, then the prompt `r3> ` on both channels. `kernel_main` has already printed `R3ATH3_STAGE rust_main`. Keyboard init has already finished before this sequence: `exceptions::init` calls `irq::enable`, which unmasks IRQ1 and sets IF. The first `r3> ` is printed immediately before the shell reads the key ring. That prompt is the ready marker: keyboard input is live.

## 8. `run` and the task language

`run_inline` splits the script on `;` bytes outside double quotes. Each segment is one task-language line, numbered from 1. `#` comments out the rest of its own segment. Blank and comment segments are skipped. A segment whose trimmed text is `run` is `reason=syntax`. An open `repeat` at the end of input is `reason=unclosed` with the line number one past the segment count. Limits are the existing ones: 24 commands, 8 variables, 72-byte segments, 48 segments, and the rest of `docs/TASK.md`. `halt` stops the script only. The shell keeps running. No heap is used. The rules are also in `docs/TASK.md`.

## 9. Memory bound

`dealloc` is a no-op, so anything allocated per key or per command would leak until reboot. The shell and `run_inline` allocate nothing per key, per line, or per command. They use stack buffers. Formatting goes through `core::fmt::Write` into the console. There is no `format!`, `String`, or `Vec` on that path. A one-time allocation at startup is allowed up to 4096 bytes. The shell does not allocate at startup either.

`mem` only reads `heap::used()`, `heap::SIZE`, and `frames::stats()`. It does not allocate or free frames.

## 10. Interrupts

The shell runs in main context with interrupts enabled. It never runs `cli`. It reads the tick counter through `irq::ticks()`. IRQ handlers do not allocate, print, or touch VGA. A fault during the shell uses the existing exception path (statuses 95–105).

## 11. Exit statuses

QEMU turns an `out` of `V` to port `0xF4` into status `(V << 1) | 1`.

| Port value | Status | Meaning |
| --- | --- | --- |
| `0x68` | 209 | `exit`, last command ok |
| `0x69` | 211 | `exit`, last command was a command error |
| `0x6A` | 213 | `exit`, last line overflowed |
| `0x6B` | 215 | `exit`, last `run` failed |
| `0x6C` | 217 | `halt` |

`0x65`–`0x67` (203–207) and `0x6D`–`0x6F` (219–223) are unused. None of these collide with 33, 35, 95, 97, 99, 101, 103, 105, 124–127, 137, 161, 163, 177, 179, 181, 193, 195, 197, 199, 201, or the scheduler statuses 225–233.

A status names an outcome. A feature that can end its own run gets its own status; shell commands are judged by exact transcript under the shell's outcome statuses, and a swap self-test proves those cases cannot pass for each other. 209 covers `shell-basic`, `shell-backspace`, `shell-run`, `shell-mem`, `shell-screen`, `shell-ps`, and `shell-wait`. 211 covers `shell-unknown`, `shell-sched-error`, and `shell-usage`. 213 covers `shell-overflow` and `shell-edge`. 215 and 217 stay one case each.

## 12. Smoke cases

`./scripts/smoke.sh` runs the previous 27 cases unchanged, then fourteen shell cases on `shell.elf`, plus the five scheduler proofs in `docs/SCHEDULER.md`. The harness uses the same QEMU command line as the monitor cases (`-vga std`, serial file, monitor socket, `-m 64M`). It deletes that case's serial log before starting QEMU. A log left by an earlier run does not count: QEMU truncates the file only after it has started, and a ready check that reads the old bytes injects keys while `init_keyboard` is still draining the controller. The harness injects no keys until this boot's log contains `R3ATH3_SHELL_READY` and the first `r3> ` prompt from section 7, and that log length stays unchanged across two reads 50 ms apart. Keys are `sendkey` 30 ms apart.

After a step is chosen, the harness records the log length **before** the first key, because the shell echoes while later keys are still in flight. It then waits, except on the last step (`exit` or `halt` does not print another prompt), until all of these hold: the bytes after that point contain the typed text followed by `\n` (a `keys` step instead needs the log to have grown and to contain a `\n`); the log ends in `r3> ` preceded by `\n` or `\x1b[H`; and the log length is unchanged across two reads 50 ms apart. The limit is 5 seconds per step. A missed wait is status 124. The `xp` step reads the VGA buffer and does not produce a prompt, so it is not a prompt-wait step. Timeouts are 20 seconds for `shell-overflow`, `shell-screen`, `shell-sched-error`, and `shell-edge`, and 12 seconds for the others.

| Case | Status | What it checks |
| --- | --- | --- |
| `shell-basic` | 209 | Empty line, `help`, `echo` collapsing spaces, `clear` ANSI sequence, `exit` with `last=ok` |
| `shell-backspace` | 209 | Leading backspace echoes nothing; editing `echo yo` with `\x08 \x08` |
| `shell-unknown` | 211 | Unknown command, then `error: usage: clear`, `last=cmd` |
| `shell-overflow` | 213 | 76 `a`s are echoed; further keys are not; the line is not run; `last=overflow` |
| `shell-run` | 209 | Inline script prints `n=5`, `hi`, `x`, `x`; a later `halt` inside `run` skips `say "no"`; the shell continues |
| `shell-run-error` | 215 | `reason=unknown` then `reason=unclosed`; the next prompt shows the shell survived; `last=script` |
| `shell-mem` | 209 | The two `mem` lines are identical, `heap_used` ≤ 4096, `frames_free == frames_total`, and the uptime fields match the tick count |
| `shell-halt` | 217 | `halt` prints `R3ATH3_SHELL_HALT` and ignores `last` |
| `shell-screen` | 209 | Serial transcript plus a 25×80 dump of `0xB8000`. After `clear`, 24 empty lines, and `echo vga`: rows 0–21 are `r3> `, row 22 is `r3> echo vga`, row 23 is `vga`, row 24 is `r3> `, every attribute `0x0A` |
| `shell-ps` | 209 | `spawn spin`, then `ps` shows shell running, idle ready, and spin ready, then `echo alive` |
| `shell-wait` | 209 | `spawn nap`, `wait 2` prints `exited id=2`, and the next `ps` shows only ids 0 and 1 |
| `shell-sched-error` | 211 | `spawn` usage, unknown demo, `wait 5` with no such task, `ps` usage, six `spin` tasks, then `error: task table full` |
| `shell-usage` | 211 | `run`, `help x`, `uptime x`, `mem x`, `exit x`, and `halt x` each print a usage error and the shell stays up |
| `shell-edge` | 213 | An exactly-76-byte `echo` line runs. 76 `a`s, then `b` and two backspaces, then Enter, still reports overflow, and the `b` is not echoed |

The screen dump is 25 lines of 80 `XXXX` hex cells. The harness self-test rejects a wrong shell status, a shortened transcript, mismatched `mem` numbers, a changed screen cell, a cell whose attribute is `0x07`, a `shell-ps` transcript judged as `shell-wait`, and a `shell-usage` transcript judged as `shell-sched-error`.

## 13. Unsafe

New `unsafe` for the console is in `vga.rs` (`with_console`, volatile cell access, and the CRTC `port::outb` calls). `idle_hlt` and `without_interrupts` are in `irq.rs`. The scheduler's `unsafe` is in `context.rs` (`docs/SCHEDULER.md`). Each new block has a `// SAFETY:` comment. `shell.rs` and `script.rs` have no `unsafe`. The file list is in `docs/FOUNDATION.md`.
