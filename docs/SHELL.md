# Interactive shell

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the `shell` feature. If the code and this document disagree, the change is not done.

The shell is a human keyboard interface. It adds no network and no model. `run` executes only the existing fixed-buffer task language. Agent requests arrive on COM1 and are decided by the gate in this task (`docs/GATE.md`). Agent bytes never enter the keyboard ring, the line editor, or `dispatch`.

## 1. Scope

In this image:

1. A `shell` feature build: a line-editing prompt `r3> ` that reads the PS/2 keyboard and echoes to COM1 and to the VGA text console.
2. A VGA text console: 80×25, cursor tracking, hardware cursor, scrolling, clear, backspace.
3. Built-ins: `help`, `echo`, `clear`, `uptime`, `mem`, `ps`, `spawn`, `wait`, `badges`, `grant`, `revoke`, `approve`, `deny`, `log`, `replay`, `selftest`, `store`, `boots`, `save`, `show`, `forget`, `run`, `exit`, `halt`.
4. Backspace, a fixed 76-byte line limit, an unknown-command error, and empty lines.
5. `run` executes an inline task-language script and survives its errors. `script` and `kbd-script` output is unchanged.

Not in this image: `ls`, `cat`, or any file system; processes or background jobs; command history; tab completion; arrow keys, Home, End, Delete, or editing mid-line; Ctrl-C or other control keys; Caps Lock; pipes, redirection, shell variables, globbing, or quoting rules in the shell itself; scrollback; colors other than green on black; keyboard input on the serial port; a mouse; a GUI or framebuffer. The default boot path is unchanged: it still prints the boot marker and exits 33. The shell build runs the kernel scheduler (`docs/SCHEDULER.md`) and the gate (`docs/GATE.md`). `spawn` starts a demo task. It is not a process, and `wait` on `spin` blocks forever because there is no Ctrl-C.

## 2. Where it plugs in

`kernel_main` order is unchanged through `mem::init()`. On the `shell` feature, `sched::init(true)` then runs, and `shell::run()` does not return. `shell` is one of the test features in the "at most one" assert and in both `boot_ok` `cfg` lists. `script` is compiled for `script`, `kbd-script`, or `shell`. The `shell` feature depends on `sched`, `gate`, `recorder`, `store`, and `nvme`. The flight recorder is specified in `docs/RECORDER.md`. The store and the emulated NVMe driver are specified in `docs/STORAGE.md`.

COM1 is the judged channel and, in this build, the agent input. It carries the prompt, echoed keys, command output, `R3ATH3_*` lines, and `R3GATE` lines. VGA shows the same transcript, except `R3ATH3_*` and `R3GATE` lines are serial-only. Line endings are `\n` only. The kernel does not echo agent bytes.

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
- The shell reads a key first. While the human line buffer is empty and not overflowed, it polls an expired gate request before it takes a buffered agent line. While the human is typing, the agent line stays buffered and is handled after Enter. At an idle prompt an agent event prints a newline first, then the human lines, then the `R3GATE` lines, then `r3> `. Agent events do not change `last`, except an approved spawn that finds the task table full, which sets `last` to cmd. When the shell is blocked in `wait`, agent lines and expiry wait until `wait` returns.

## 7. Built-ins

Output lines end in `\n` and go to both channels unless marked serial-only.

| Command | Arguments | Output | `last` |
| --- | --- | --- | --- |
| `help` | none | The 24 lines below | ok |
| `echo` | any | The words joined by single spaces. `echo` alone prints an empty line. Quotes are not special. | ok |
| `clear` | none | Serial `\x1b[2J\x1b[H`; VGA clear | ok |
| `uptime` | none | `uptime ticks=<n> hz=100 seconds=<s>.<cc>` with `n = irq::ticks()`, `s = n / 100`, and `cc = n % 100` as two digits | ok |
| `mem` | none | `mem heap_used=<heap::used()> heap_size=1048576 frames_total=<t> frames_free=<f>` from `frames::stats()` | ok |
| `ps` | none | One line per non-free slot, id order: `task id=<n> name=<name> state=<ready\|running\|sleeping\|blocked\|exited>` | ok |
| `spawn` | one word, `spin` or `nap` | `spawned id=<n> name=<demo>` | ok |
| `wait` | one decimal id | `exited id=<n>` after the slot is reaped | ok |
| `badges` | none | One `badge` line per badge, then the pending request or `no pending request` | ok |
| `grant` | `AGENT PERM` | `granted <A> <P>` | ok |
| `revoke` | `AGENT PERM` or `AGENT all` | `revoked <A> <P\|all>`, then any cancellation | ok |
| `approve` | one request number | The approved line and the `R3GATE reply` | ok |
| `deny` | one request number | `gate: request <n> denied` and the `R3GATE reply` | ok |
| `log` | none | `log entries=<len> of 128 lost=<lost> head=<16 hex>`, then the newest `min(5, len)` entries | ok |
| `replay` | none, or one entry number | The verified record, or one verified entry. See `docs/RECORDER.md` | ok |
| `selftest` | none | The eight-check report card | ok, or cmd if a check fails |
| `store` | none | Device line, then what this boot is | ok |
| `boots` | none, or one boot number | One line per non-free record, then the summary, or that record's entries | ok, or cmd on error |
| `save` | `NAME TEXT` | `saved <NAME> to ram` on the RAM disk, `saved <NAME> to nvme` on the emulated NVMe disk | ok |
| `show` | none, or one name | Saved names, or `<NAME> = <TEXT>` | ok |
| `forget` | one name | `forgot <NAME>` | ok |
| `app` | none, or one name | No argument lists the baked apps. A name runs that app. See `docs/WASM.md` | ok for a list, a quit, or a trap; cmd for usage, an unknown name, an ABI failure, or a module that does not load |
| `run` | the rest of the line after `run` and its following spaces, raw | Script lines (`text` plus `\n`) on both channels. On error: `error: script line=<n> reason=<word>` | ok, or script on error |
| `exit` | none | Serial-only `R3ATH3_SHELL_EXIT last=<ok\|cmd\|overflow\|script>`, then the status in section 8 | n/a |
| `halt` | none | Serial-only `R3ATH3_SHELL_HALT`, then status 217, whatever `last` is | n/a |

An unknown first word prints `error: unknown command: <word>` and sets `last` to cmd. Extra arguments to `help`, `clear`, `uptime`, `mem`, `ps`, `badges`, `exit`, or `halt`, or `run` with nothing after it, print `error: usage: <command>` (`error: usage: run SCRIPT` for `run`, `error: usage: ps` for `ps`, `error: usage: badges` for `badges`). `spawn` with no word or extra words prints `error: usage: spawn DEMO`. Any other demo word prints `error: unknown demo: <word>`. A full table prints `error: task table full`. `wait` with a missing or extra argument prints `error: usage: wait ID`. A non-number, a value outside 2–7, or a free slot prints `error: no task <arg>` using the raw argument. `grant` and `revoke` with the wrong number of words print `error: usage: grant AGENT PERM` and `error: usage: revoke AGENT PERM`. A bad name prints `error: bad agent name: <A>`. An unknown permission prints `error: unknown permission: <P>`. A new name when four badges exist prints `error: badge table full`. `revoke` of an unknown name prints `error: no badge <A>`. `approve` and `deny` with the wrong number of words print `error: usage: approve N` and `error: usage: deny N`. A full flight recorder prints `error: flight recorder full: approve refused` before the number is parsed. A number that is not the pending request prints `error: no pending request <N>`. `log` with an argument prints `error: usage: log`. `replay` with more than one argument prints `error: usage: replay [N]`. A `replay` number that is not a stored entry prints `error: no log entry <N>`. A broken chain prints `replay: chain broken at #<k>`. `selftest` with an argument prints `error: usage: selftest`. `store` with an argument prints `error: usage: store`. `boots` with two or more words prints `error: usage: boots [K]`. A `boots` word that is not a stored boot prints `error: no boot <word>`. A bad header prints `boot <K>: bad header`. A broken stored chain prints `boot <K>: chain broken at #<j>`. `save` without a name or without text prints `error: usage: save NAME TEXT`. A name that is not 1–12 bytes of `[a-z0-9-]` prints `error: bad saved name: <NAME>`. Text longer than 58 bytes prints `error: saved text too long (max 58)`. A full saved area prints `error: saved data full (16 names)`. `show` with two or more words prints `error: usage: show [NAME]`. `forget` with no word or with extra words prints `error: usage: forget NAME`. `app` with two or more words prints `error: usage: app [NAME]`. An unknown name prints `error: no app <NAME>`. Names match exactly (`Spin` is not `spin`). A quit and a trap both set `last` to ok, so `exit` after either one is status 209. An ABI failure prints `app <NAME>: stopped: abi` and a module that does not load prints `app <NAME>: not loaded: <word>`; both set `last` to cmd. The end lines and the screen rules are in `docs/WASM.md`. A missing saved name prints `error: no saved name <NAME>`. A store read failure prints `error: store read failed`. A `save` or `forget` write failure prints `error: store write failed`. Line formats for the recorder are in `docs/RECORDER.md`. The store formats are in `docs/STORAGE.md`. The name is checked before the permission. Each of those errors sets `last` to cmd. The shell does not exit. `spin` loops and never returns. `nap` sleeps 20 ticks and returns. `wait` on `spin` blocks forever. Gate line formats, reasons, and the verb table are in `docs/GATE.md`.

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
badges      list AI badges and the waiting request
grant A P   give agent A permission P
revoke A P  take permission P (or all) from agent A
approve N   let gate request N run
deny N      refuse gate request N
log         newest flight-recorder entries and totals
replay [N]  check the record and print it, or entry N
selftest    run the feature checks and print a report card
store       storage device and what survives a reboot
boots [K]   list saved boot records, or print boot K
save N TEXT keep TEXT under name N
show [N]    list saved names, or the text saved as N
forget N    remove the text saved as N
app [NAME]  list built-in WASM apps, or start app NAME
run SCRIPT  run a task script, lines separated by ;
exit, halt  leave with the last status, or stop the machine
```

Startup, in order: serial-only `R3ATH3_SHELL_READY`, a VGA clear with no ANSI sequence, then `R3@TH3 shell. Type help for commands.` on both channels, then the store mount, then the prompt `r3> ` on both channels. With no NVMe controller the mount is silent and uses the RAM disk, and a reboot erases that disk. With QEMU's emulated NVMe disk the mount prints the `store:` lines in `docs/STORAGE.md` on both channels. Those lines are not recorded. `kernel_main` has already printed `R3ATH3_STAGE rust_main`. Keyboard init has already finished before this sequence: `exceptions::init` calls `irq::enable`, which unmasks IRQ1 and sets IF. The first `r3> ` is printed immediately before the shell reads the key ring. That prompt is the ready marker: keyboard input is live.

## 8. `run` and the task language

`run_inline` splits the script on `;` bytes outside double quotes. Each segment is one task-language line, numbered from 1. `#` comments out the rest of its own segment. Blank and comment segments are skipped. A segment whose trimmed text is `run` is `reason=syntax`. An open `repeat` at the end of input is `reason=unclosed` with the line number one past the segment count. Limits are the existing ones: 24 commands, 8 variables, 72-byte segments, 48 segments, and the rest of `docs/TASK.md`. `halt` stops the script only. The shell keeps running. No heap is used. The rules are also in `docs/TASK.md`.

## 9. Memory bound

`dealloc` is a no-op, so anything allocated per key or per command would leak until reboot. The shell and `run_inline` allocate nothing per key, per line, or per command. They use stack buffers. Formatting goes through `core::fmt::Write` into the console. There is no `format!`, `String`, or `Vec` on that path. A one-time allocation at startup is allowed up to 4096 bytes. The shell does not allocate at startup either.

`mem` only reads `heap::used()`, `heap::SIZE`, and `frames::stats()`. It does not allocate or free frames. `selftest` allocates and frees one frame, and the counts are restored before it returns. `mem` still only reads.

## 10. Interrupts

The shell runs in main context with interrupts enabled. It never runs `cli` itself. It reaches IF-clear sections only through `IrqCell` (the scheduler table, the flight recorder, the store state, the RAM disk, the NVMe controller, and the NVMe refusal word). It reads the tick counter through `irq::ticks()`. IRQ handlers do not allocate, print, or touch VGA. A fault during the shell uses the existing exception path (statuses 95–105).

## 11. Exit statuses

QEMU turns an `out` of `V` to port `0xF4` into status `(V << 1) | 1`.

| Port value | Status | Meaning |
| --- | --- | --- |
| `0x68` | 209 | `exit`, last command ok |
| `0x69` | 211 | `exit`, last command was a command error |
| `0x6A` | 213 | `exit`, last line overflowed |
| `0x6B` | 215 | `exit`, last `run` failed |
| `0x6C` | 217 | `halt` |

`0x65`–`0x67` (203–207), `0x6D`–`0x6F` (219–223), and `0x76`–`0x77` (237–239) are unused, as is `0x7F` (255). None of these collide with 33, 35, 95, 97, 99, 101, 103, 105, 124–127, 137, 161, 163, 177, 179, 181, 193, 195, 197, 199, 201, or the scheduler statuses 225–233 and 235, or the gate statuses 241 and 243, or the recorder status 245, or the store status 247, or the NVMe status 249, or the WebAssembly statuses 251 and 253.

A status names an outcome. A feature that can end its own run gets its own status; shell commands are judged by exact transcript under the shell's outcome statuses, and a swap self-test proves those cases cannot pass for each other. 209 covers `shell-basic`, `shell-backspace`, `shell-run`, `shell-mem`, `shell-screen`, `shell-ps`, `shell-wait`, `gate-approve`, `gate-noperm`, `gate-revoke`, `log-record`, `log-full`, `shell-selftest`, `store-ram`, `store-reboot`, `store-baddisk`, `app-hello`, `app-spin`, `app-gate`, and `app-glue` (20). 211 covers `shell-unknown`, `shell-sched-error`, `shell-usage`, `gate-sneak`, `gate-usage`, `shell-sched-args`, `log-usage`, `store-usage`, `store-tamper`, and `app-usage` (10). `store-tamper`'s first boot also exits 209. 213 covers `shell-overflow` and `shell-edge`. 215 stays one case. 217 covers `shell-halt` and `gate-halt`. The NVMe proof `nvme-core` exits 249. The WebAssembly proof `wasm-core` exits 251. The Glue Duty proof `wasm-glue` exits 253. The unused odd statuses are 203, 205, 207, 219, 221, 223, 237, 239, and 255.

## 12. Smoke cases

`./scripts/smoke.sh` runs 75 QEMU cases. The cases in the table below run on `shell.elf` (35 of them, including `store-ram`, `store-usage`, `store-reboot`, `store-tamper`, `store-baddisk`, `app-hello`, `app-glue`, `app-spin`, `app-usage`, and `app-gate`). With no disk the shell prints nothing between the banner and the first prompt. With QEMU's emulated NVMe disk it prints the `store:` lines in `docs/STORAGE.md` before that prompt. Those lines are not recorded. `recorder-core`, `store-core`, `nvme-core`, `wasm-core`, and `wasm-glue` are their own builds. `wasm-core` exits 251 and `wasm-glue` exits 253. Their logs are in `docs/WASM.md`. Scheduler proofs are in `docs/SCHEDULER.md`. Gate proofs and `expect_gate` are in `docs/GATE.md`. The recorder proof and the `log` judge are in `docs/RECORDER.md`. The store proof and the `store` judge are in `docs/STORAGE.md`. Keyboard cases use the monitor (`-vga std`, serial file, monitor socket, `-m 64M`). The harness deletes that case's serial log before starting QEMU. A log left by an earlier run does not count: QEMU truncates the file only after it has started, and a ready check that reads the old bytes injects keys while `init_keyboard` is still draining the controller. The harness injects no keys until this boot's log contains `R3ATH3_SHELL_READY` and the first `r3> ` prompt from section 7, and that log length stays unchanged across two reads 50 ms apart. Keys are `sendkey` 30 ms apart. Gate cases use a serial socket with `wait=on` and connect to it before the monitor, so the boot lines are not lost.

After a step is chosen, the harness records the log length **before** the first key, because the shell echoes while later keys are still in flight. It then waits, except on the last step (`exit` or `halt` does not print another prompt), until all of these hold: the bytes after that point contain the typed text followed by `\n` (a `keys` step instead needs the log to have grown and to contain a `\n`); the log ends in `r3> ` preceded by `\n` or `\x1b[H`; and the log length is unchanged across two reads 50 ms apart. The limit is 5 seconds per step. A missed wait is status 124. The `xp` step reads the VGA buffer and does not produce a prompt, so it is not a prompt-wait step. `launch NAME` types `app NAME` and waits for `R3ATH3_APP_READY name=NAME`. `appquit` sends `q` and waits until the end line is in the log and the prompt is back. `appkey K N` sends one key and waits for N later `R3ATH3_APP_STATE` lines, or 3 seconds when N is 0. Part A uses `launch` and `appquit` for `app-hello`. An `ai` step writes the request line on the serial socket and waits until a `R3GATE reply ` line has arrived and the log is idle at `r3> `, with the same 5 second limit. Timeouts are 60 seconds for `log-full`, 20 seconds for `shell-overflow`, `shell-screen`, `shell-sched-error`, `shell-edge`, `gate-usage`, and `log-record`, and 12 seconds for the others. No shell case waits 60 seconds for a gate expiry. `log-record` and `log-full` are judged by the `log` judge, which masks `t=`, `link=`, and `head=` on header and summary lines and recomputes them. `store-ram` is judged by the `store` judge (`store:s1`), which masks the same fields and also checks that boot 1 agrees with the live record. `store-usage` is exact. Its limit is 40 seconds. `store-reboot` and `store-tamper` use the store judge, and boot 2 is checked against boot 1. `store-baddisk` is exact, and the harness also checks that the image sha256 is unchanged. Every other shell case stays exact.

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
| `shell-sched-args` | 211 | `wait` with no argument and with an extra argument, `wait abc`, `wait 99`, and `spawn a b` |
| `gate-approve` | 209 | `spawn nap` does nothing while pending, then runs once after `approve 1`, and `wait 2` reaps it |
| `gate-noperm` | 209 | No badge, then no spawn permission, then `read` returns the three work orders |
| `gate-revoke` | 209 | Revoking `spawn` cancels the pending request in that command; `say` still works; `revoke all` removes the badge |
| `gate-sneak` | 211 | Bad lines, `busy`, a denied `spin` that never runs, and a number that cannot be approved again |
| `gate-halt` | 217 | Approve of `halt` prints the reply, then `R3ATH3_SHELL_HALT` |
| `gate-usage` | 211 | Usage, bad names, unknown permissions, and a full badge table. 20 seconds |
| `log-record` | 209 | Gate outcomes, human ok and cmd lines, `replay`, `replay N`, and `log`. Judged by the `log` judge. 20 seconds |
| `log-full` | 209 | Reserve refusal (`reason=log-full`), one full notice, `approve` refused, `deny` allowed, `lost` counted, 128-entry `replay`. 60 seconds |
| `log-usage` | 211 | `log`, `replay`, and `selftest` usage errors, and `replay` of a missing entry |
| `shell-selftest` | 209 | All eight checks pass. `badges` and `ps` are unchanged afterward |
| `store-ram` | 209 | Save, show, forget, `boots`, and `replay` of this boot. The saved copy matches the live record. 20 seconds |
| `store-usage` | 211 | Every store error line, 16 saved names, and a refused 17th. 40 seconds |
| `app-hello` | 209 | `hello` paints `hello from wasm` at row 12 column 30 and `press q to quit` at row 14 column 30, then `app hello: quit`. 20 seconds |
| `app-glue` | 209 | Title screen matches the golden. Space, pause, resume, then `app glue-duty: quit; shift 1 score 0 cleaned 0 gum-up 0%`. 20 seconds |
| `app-spin` | 209 | The three-app list, then `app spin: stopped: trap fuel`, then `echo alive` |
| `app-usage` | 211 | `app a b`, `app nope`, and `app Spin` |
| `app-gate` | 209 | `grant ai1 app` is an unknown permission; an agent `app` is `reason=verb`; a `say` of `app spin` is text and does not start an app; the human `app spin` traps `fuel`; badges are unchanged |

`app-hello`, `app-glue`, and `app-spin` use a 20 second limit. The screen dump is 25 lines of 80 `XXXX` hex cells. The harness self-test also rejects a `wasm-core` log with one step changed, with the `ok` line removed, or judged at status 249; an `app-hello` dump with one cell changed; an `app-spin` log whose end line says `trap stack`; an `app-gate` log with `R3ATH3_APP_READY` after the agent's `say`; a `wasm-glue` replay or golden whose `frame=` differs; a truncated `wasm-glue` kernel log; and an `app-glue` dump with one cell changed. After both proofs have run, `wasm-core`'s log judged as `nvme-core` fails and the reverse fails, including when each log is judged at the other proof's own status so the check is the body and not only the status. `wasm-glue` judged as `wasm-core` fails. The harness self-test rejects a wrong shell status, a shortened transcript, mismatched `mem` numbers, a changed screen cell, a cell whose attribute is `0x07`, a `shell-ps` transcript judged as `shell-wait`, and a `shell-usage` transcript judged as `shell-sched-error`. It also rejects each new transcript at the wrong status, and these swaps: `gate-approve` against `gate-noperm`, `gate-revoke` against `gate-approve`, `gate-sneak` against `gate-usage`, `shell-sched-args` against `shell-sched-error`, and `gate-halt` against `shell-halt`. The `log` judge's self-test is in `docs/RECORDER.md`: a changed link, a changed body byte, decreasing ticks, a substituted head, an inconsistent reprint, a gap, the correct sample judged at 211, and the shared-status swaps of `log-record`, `log-full`, `log-usage`, and `shell-selftest`. The `store` judge's self-test is in `docs/STORAGE.md`, including the swaps of `store-usage` against `log-usage` and `shell-usage`.

## 13. Unsafe

New `unsafe` for the console is in `vga.rs` (`with_console`, volatile cell access, and the CRTC `port::outb` calls). `idle_hlt` and `without_interrupts` are in `irq.rs`. The scheduler's `unsafe` is in `context.rs` (`docs/SCHEDULER.md`). Each new block has a `// SAFETY:` comment. `shell.rs`, `script.rs`, `gate.rs`, `gatetest.rs`, `recorder.rs`, `recordertest.rs`, `selftest.rs`, `block.rs`, `ramdisk.rs`, `store.rs`, and `storetest.rs` have no `unsafe`. The file list is in `docs/FOUNDATION.md`.
