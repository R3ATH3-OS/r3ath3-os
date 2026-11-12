# Kernel task scheduler

## 1. Purpose

One CPU runs several tasks, cooperatively or one tick at a time, so the shell can start a demo and wait for it to finish.

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the scheduler. If the code and this document disagree, the change is not done.

There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0, target `x86_64-unknown-none`.

## 2. Scope

In this image:

1. A fixed 8-slot task table, round-robin, with cooperative `yield_now` and PIT preemption (quantum 1 tick, 10 ms).
2. States `ready`, `running`, `sleeping`, `blocked`, and `exited`. `sleep_ticks(n)` and `sleep_until(t)`. `wait(id)` blocks until that task has exited, then reaps the slot.
3. An idle task that runs only when nothing else is ready and calls `irq::idle_hlt`.
4. Seven 16 KiB stacks in a static `.bss` pool, with a canary checked on every switch.
5. Shell built-ins `ps`, `spawn DEMO` (`spin`, `nap`), and `wait ID`.
6. Six `sched-*` proofs, each with its own QEMU status.

Not in this image, and not added here: processes, address spaces, user mode (ring 3), memory protection, guard pages, page-table changes, SMP or APIC, priorities, per-task time accounting beyond the counters below, a mutex, semaphore, or channel, `kill`, Ctrl-C, background shell jobs that print, a growing task table, heap-allocated stacks, FPU/SSE context, unmasking any IRQ besides 0 and 1, a model runtime, and the audit log. The human gate is milestone 8 (`docs/GATE.md`). The default boot path is unchanged: boot marker, status 33.

## 3. Modules

| File | Kind | Role |
| --- | --- | --- |
| `kernel/src/context.rs` | driver | Switch and trampoline `global_asm!`, the stack pool and canaries, the saved-RSP table, initial frames, `IrqCell<T>`. All new scheduler `unsafe` is here. |
| `kernel/src/sched.rs` | safe | Task table, round-robin, `spawn`, `yield_now`, `sleep_ticks`, `sleep_until`, `wait`, `exit_current`, `on_tick`, `snapshot`. |
| `kernel/src/schedtest.rs` | safe | The six `sched-*` bodies. |
| `kernel/src/irq.rs` | driver | Timer hook, `without_interrupts`, the `KbData` safety comment. `idle_hlt` and `ticks` are also compiled for `feature = "sched"`. |
| `kernel/src/shell.rs` | safe | `ps`, `spawn`, `wait`, and the gate commands in `docs/GATE.md`. The key loop calls `sched::yield_now`. |
| `kernel/src/qemu_exit.rs` | driver | Statuses 225, 227, 229, 231, 233, and 235. |

`context` and `sched` are compiled only with `feature = "sched"`. `sched` is internal: `shell = ["sched"]` and each `sched-*` feature depends on it. `sched` itself is not one of the "at most one" test features. A build without `sched` does not contain this code. The 27 non-shell cases are that proof.

## 4. Where it plugs in

`kernel_main` is unchanged through `mem::init()`. On success, a `sched` build calls `sched::init(preempt)` before the feature dispatch. `preempt` is false for `sched-coop`, `sched-full`, and `sched-reap`, and true for every other `sched` build, including `shell`. `init` prints nothing. It adopts the running boot context as task 0 (`main`, or `shell` in the shell build) on the boot stack, marks it `running`, prepares idle in slot 1 as `ready`, and sets the table live.

The PIT is already running when `init` is called, because `exceptions::init` unmasked IRQ0. `on_tick` returns immediately until the table is live, so a tick during `mem::init` does not switch on an empty table.

The five `sched-*` features are in the "at most one" assert and in both `boot_ok` `cfg` lists. `sched` itself is not.

## 5. Mechanism

### Task table

`MAX_TASKS = 8`. The slot index is the task id. Slot 0 is the boot context. Slot 1 is `idle`. Slots 2–7 are spawnable (six tasks).

`Task` holds `state`, `name: &'static str`, `wake_at: u64`, `waiting_on: Option<usize>`, `entry: fn(usize)`, and `arg: usize`. `State` is `Free`, `Ready`, `Running`, `Sleeping`, `Blocked`, or `Exited`. The words `ps` prints are `ready`, `running`, `sleeping`, `blocked`, and `exited`. `Free` is not printed.

Exactly one task is `Running`. The table lives in `context::IrqCell<Table>`. Every borrow runs with IF clear.

`spawn(name, entry, arg) -> Result<usize, SpawnError>` takes the lowest `Free` slot in 2–7, writes its initial frame, and sets `Ready`. It returns `SpawnError::Full` when none is free. It does not switch.

`pick_next(current)` scans `current+1`, `current+2`, … modulo 8, skips slot 1, and takes the first `Ready` task. The current task counts when it is still `Ready`. If none is ready, the scan returns idle. That fixed order is what makes the cooperative interleaving deterministic.

- `yield_now`: current → `Ready`, then switch to `pick_next`. If that is the current task, it is set back to `Running` and no switch happens.
- `sleep_until(t)`: if `t` is already `<= ticks()` the call returns. Otherwise current → `Sleeping`, `wake_at = t`, and switch. `ticks` is sampled with IF clear.
- `sleep_ticks(n)`: `n == 0` returns. Otherwise `t = ticks().wrapping_add(n)`, sampled with IF clear, then the same path as `sleep_until`.
- `wait(id)` returns `Result<bool, WaitError>`. `id` must be 2–7 and not `Free`, else `Err(NoTask)`. If the slot is `Exited`, it is reaped (`Free`) and the call returns `Ok(true)`. Otherwise current → `Blocked`, `waiting_on = id`, the generation is recorded, and switch. On wake the slot is reaped only through `reap_on_wake` when it is still `Exited` and the generation matches. `Ok(true)` means this call reaped the slot. `Ok(false)` means another waiter already reaped it. A waiter reaps on wake only if the slot still holds the same incarnation it waited for (a per-slot generation that `spawn` increments).
- `exit_current` (the trampoline calls `sched_task_exit`, which calls this): state → `Exited`, every task `Blocked` on this id becomes `Ready`, then switch away. It does not return. An exited task keeps its slot until `wait` reaps it. Slots 0 and 1 never exit.

`on_tick` runs from the IRQ0 handler, with IF clear, after EOI. It does not allocate, print, or touch VGA. Every `Sleeping` task with `wake_at <= ticks()` becomes `Ready`. If preemption is on and the current task is not idle, the current task becomes `Ready` and the hook does `yield_now`'s switch. `PREEMPTIONS` increments only when that switch actually leaves a non-idle task. If idle is running and any sleeper became ready, idle yields too, and that switch does not count as a preemption.

The mutable borrow of the table ends before `context::switch`. The switch still runs inside the same `irq::without_interrupts` guard, so IF stays clear across it, and two tasks never hold `&mut Table` at once. A cooperative guard restores the caller's IF when it returns on resume. A preemptive switch returns into the IRQ stub, and `iretq` restores IF.

Idle (slot 1) is `loop { IDLE_RUNS += 1; irq::idle_hlt(); sched::yield_now(); }`. The trampoline has already run `sti`, so idle halts with IF set. It runs only when no other slot is `Ready`.

Before every switch away from slot 1–7, `context::check_canary` reads the canary. A mismatch panics `task stack overflow id=<n>` (status 35). Slot 0 uses the boot stack and has no canary.

`SWITCHES` counts switches where the source and destination differ. `PREEMPTIONS` and `IDLE_RUNS` are the counters above. None of them is an exact value a judge compares, except `SWITCHES >= 9` on the cooperative proof.

### Timer hook

Vector 32 does `TICKS.fetch_add(1)`, then `eoi(0)`, then `sched::on_tick` when `sched` is enabled. EOI comes first because a switch inside the hook can run another task for a full quantum before this handler returns. Builds without `sched` keep the old handler: increment, then EOI, and no hook.

`irq::without_interrupts` is `pushfq`/`pop`, `cli`, run the closure, then `sti` only if IF was set. It is the only way safe code disables interrupts. Nesting is safe: an inner call sees IF clear and does not set it.

### `IrqCell` and the context switch

`IrqCell<T>` is `UnsafeCell<T>` plus `unsafe impl Sync`. `with` runs the closure through `irq::without_interrupts` and does not let the `&mut` escape. Safety: one CPU, and every access takes that path, so the IRQ0 hook and task code never overlap.

`switch_context(old: *mut u64, new: u64)` lives in `.text.context` (each `global_asm!` block has to name its own section; the boot stub's block leaves the assembler in `.bss.boot`). It pushes `rbp rbx r12 r13 r14 r15`, stores RSP through `old`, loads `new` into RSP, pops the six, and `ret`. The safe wrapper `context::switch(from, to)` indexes the saved-RSP table. Callers hold IF clear. Debug builds assert IF is clear via `pushfq`. IF is not part of the saved frame.

A new task's frame sits on a 16-byte-aligned `top`: `[top-8] = task_trampoline`, then six slots with `r12 = entry` and `r13 = arg`, and the saved RSP is `top - 56`. `task_trampoline` is `sti; cld; mov %r13, %rdi; call *%r12; call sched_task_exit; ud2`. At `call *%r12`, RSP is `top`, 16-byte aligned, which is what the SysV ABI requires. `entry` is `fn(usize)`; the integer argument arrives in `rdi`.

Stacks: a `#[repr(C, align(16))]` pool of 7 × 16 KiB (slots 1–7) in `.bss`, 112 KiB. `heap_used` does not change, and the frame allocator does not hand the pages out because they are inside the kernel image. `prepare` writes the canary `0x5233_4154_4833_5354` at the lowest 8 bytes.

The `x86_64-unknown-none` target builds this kernel with SSE/MMX off (soft-float). The switch does not save FPU or SSE state. Function bodies in the `sched-preempt` image contain no `xmm`, `ymm`, or `fxsave` instruction. `.rodata` is linked into the executable segment, so a linear disassembly of that segment can decode a function-pointer word as `movups`; that word is not an instruction.

Task lines that smoke judges are written inside `irq::without_interrupts`, so a preemption cannot split a line. The shell is the only task that reads keys or touches the VGA console. `spin` and `nap` print nothing. In a `sched` build the shell task is the only caller of `irq::pop_key`. The `keys` and `kbd-script` builds have no scheduler and read the ring from the boot context.

## 6. Shell

The key loop calls `sched::yield_now` instead of `irq::idle_hlt`. With nothing else ready, `pick_next` returns the shell and `yield_now` returns at once, so a key is noticed within one tick (10 ms). With six `spin` tasks the shell runs once per round of quanta, at most 70 ms. When the shell is blocked in `wait` and nothing else is ready, idle halts.

`help` is 16 lines. These three are inserted after `mem`. The five gate lines sit after `wait ID` (`docs/GATE.md`, `docs/SHELL.md`):

```
ps          list tasks
spawn DEMO  start demo task spin or nap
wait ID     wait for a task to exit
```

`shell-basic`'s expected help text includes them. It is the only pre-existing expected transcript that changes.

`ps` with no arguments prints one line per non-`Free` slot, in id order: `task id=<n> name=<name> state=<ready|running|sleeping|blocked|exited>`. Slot 0 is named `shell`. `last` becomes ok. Extra arguments print `error: usage: ps` and set `last` to cmd.

`spawn DEMO` with exactly one word starts a demo. `spin` loops on an `AtomicU64` increment and `spin_loop`, and never yields or returns. `nap` calls `sleep_ticks(20)` and returns. Success prints `spawned id=<n> name=<demo>`. No word, or extra words: `error: usage: spawn DEMO`. Any other word: `error: unknown demo: <word>`. A full table: `error: task table full`. Each error sets `last` to cmd.

`wait ID` takes one decimal word. It prints `exited id=<n>` after the reap. A missing or extra argument prints `error: usage: wait ID`. A non-number, a value outside 2–7, or a `Free` slot prints `error: no task <arg>` and sets `last` to cmd. `wait` on `spin` blocks forever. There is no Ctrl-C.

No shell path allocates. `shell-mem` keeps `heap_used <= 4096`.

## 7. Proofs

Every line is `R3ATH3_SCHED_*`. An internal check that fails panics (status 35), which no case accepts. The boot task waits for every task it spawned, except the preempt proof's `hog`, which is still running when QEMU exits.

1. **`sched-coop` (preempt off).** Spawn A, B, and C in slots 2–4. Each prints `R3ATH3_SCHED_COOP task=<X> step=<k>` for `k` in 1..=3 and then `yield_now`. The boot task waits on 2, 3, and 4, then prints `R3ATH3_SCHED_COOP ok order=ABCABCABC switches=<n>`.
2. **`sched-preempt` (preempt on).** `hog` in slot 2 increments a counter forever and never yields or prints. `worker` in slot 3 prints `R3ATH3_SCHED_PREEMPT worker step=<k>` five times and `sleep_ticks(1)` after each print. The boot task waits on 3, then prints `R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=<h> preemptions=<p>`.
3. **`sched-sleep` (preempt on).** `start = ticks()`. Spawn `L`, `M`, and `S` to `sleep_until(start + 30/20/10)`. On wake each prints `R3ATH3_SCHED_SLEEP name=<X> want=<w> elapsed=<ticks()-start>`. After all three are waited, the boot task prints `R3ATH3_SCHED_SLEEP ok count=3 idle_runs=<i>`.
4. **`sched-full` (preempt off).** Six empty tasks spawn as ids 2–7. The seventh returns `Full`. After those six are waited, one more spawn returns id 2. The line is `R3ATH3_SCHED_FULL ok spawned=6 error=full after_reap=2`.
5. **`sched-exit` (preempt on).** Two rounds of six tasks that increment `RAN` and return, each round waited. `heap::used` is recorded before round 1. The line is `R3ATH3_SCHED_EXIT ok rounds=2 spawned=12 ran=12 free=6 heap_delta=0`. `free` is the number of `Free` slots in 2–7. `heap_delta` is `heap::used()` minus the recorded value. A mismatch panics instead of printing that line.
6. **`sched-reap` (preempt off).** `T` in slot 2 yields once and returns. `W1` in slot 3 and `W2` in slot 4 both `wait(2)`. The boot task waits on 3, then 4. `W1` reaps (`Ok(true)`). `W2` finds the slot `Free` (`Ok(false)`). The line is `R3ATH3_SCHED_REAP waiters=2 reaped_by=W1 w2_reaped=no free=6`. Then `selftest_stale_reap` prints `R3ATH3_SCHED_REAP stale_gen=kept same_gen=reaped`, then `R3ATH3_SCHED_REAP ok`. A local table holds slot 2 `Exited` at generation 5: a recorded generation of 4 keeps it `Exited`, and 5 reaps it.

## 8. Statuses

| Port value | Status | Meaning | Helper |
| --- | --- | --- | --- |
| `0x70` | 225 | `sched-coop` ok | `sched_coop_ok` |
| `0x71` | 227 | `sched-preempt` ok | `sched_preempt_ok` |
| `0x72` | 229 | `sched-sleep` ok | `sched_sleep_ok` |
| `0x73` | 231 | `sched-full` ok | `sched_full_ok` |
| `0x74` | 233 | `sched-exit` ok | `sched_exit_ok` |
| `0x75` | 235 | `sched-reap` ok | `sched_reap_ok` |

No collision with 33, 35, 95, 97, 99, 101, 103, 105, 124–127, 137, 161, 163, 177, 179, 181, 193, 195, 197, 199, 201, 209–217, or the gate statuses 241 and 243. `0x65`–`0x67` (203–207), `0x6D`–`0x6F` (219–223), and `0x76`–`0x77` (237–239) stay unused. Each helper is compiled only for its feature. The shell keeps 209–217. New shell cases use those outcome statuses.

A status names an outcome. A feature that can end its own run gets its own status. Shell commands are judged by exact transcript under the shell's outcome statuses, and a swap self-test proves those cases cannot pass for each other.

## 9. Determinism

TCG drives the PIT from the host clock. Ticks arrive in bursts or gaps, and the instructions per tick vary. No judge compares an exact tick count, spin count, or preemption count.

- Order fixed by construction: cooperative mode with the fixed slot scan (`sched-coop`, `sched-full`). Absolute deadlines from one `start` (`sched-sleep`: ticks only increase, so `S` wakes before `M` before `L`).
- Exact counts the kernel controls: steps, spawns, reaps, free slots, heap delta.
- Lower bounds only correct behavior can meet: `elapsed >= want`, `preemptions >= 5` (each of the worker's five steps runs only after `hog` is preempted), `hog_spins > 0`, `idle_runs > 0`.
- A loose upper bound: `elapsed <= want + 100` (1 s). It catches a task that never sleeps.
- A missing preemption shows up as a timeout (124), not as a pass.

## 10. Smoke

`./scripts/smoke.sh` keeps every previous judge. Only `shell-basic`'s expected help text changes. The scheduler builds (`sched-coop`, `sched-preempt`, `sched-sleep`, `sched-full`, `sched-exit`, `sched-reap`) use `expect_case` / `run_qemu` (`-vga none`, `-serial stdio`, 12 s). Shell cases use `expect_shell` on `shell.elf`. Gate cases use `expect_gate` (`docs/GATE.md`).

`P` below is the prompt `r3> `. Shell transcripts start with the usual three-line prefix plus that prompt.

| Case | Status | Judge |
| --- | --- | --- |
| `sched-coop` | 225 | Stage line, the nine step lines in order A1 B1 C1 A2 B2 C2 A3 B3 C3, then the `ok` line whose `switches=` matches `[0-9]+` and is ≥ 9. |
| `sched-preempt` | 227 | Exactly five `worker step=1..5` lines in order, then the `ok` line. `hog_spins > 0`. `preemptions >= 5`. |
| `sched-sleep` | 229 | Exactly three wake lines in order S, M, L with `want` 10, 20, 30. Each `want <= elapsed <= want+100`. `ok count=3`. `idle_runs > 0`. |
| `sched-full` | 231 | Stage line plus the exact `ok` line. |
| `sched-exit` | 233 | Stage line plus the exact `ok` line. |
| `sched-reap` | 235 | Stage line, `R3ATH3_SCHED_REAP waiters=2 reaped_by=W1 w2_reaped=no free=6`, `R3ATH3_SCHED_REAP stale_gen=kept same_gen=reaped`, then `R3ATH3_SCHED_REAP ok`. |
| `shell-ps` | 209 | Exact. `spawn spin` → `spawned id=2 name=spin`. `ps` → shell `running`, idle `ready`, spin `ready`. `echo alive` → `alive`. `exit` → `last=ok`. |
| `shell-wait` | 209 | Exact. `spawn nap` → `spawned id=2 name=nap`. `wait 2` → `exited id=2`, whether `nap` had already exited or not. `ps` shows only ids 0 and 1. `exit` → `last=ok`. |
| `shell-sched-error` | 211 | Exact, 20 s. `spawn` → usage. `spawn foo` → `error: unknown demo: foo`. `wait 5` → `error: no task 5`. `ps x` → `error: usage: ps`. Six `spawn spin` → ids 2–7. A seventh → `error: task table full`. `exit` → `last=cmd`. |
| `shell-usage` | 211 | Exact. `run` → `error: usage: run SCRIPT`. `help x`, `uptime x`, `mem x`, `exit x`, and `halt x` each print `error: usage: <cmd>`, and the shell stays up. `exit` → `last=cmd`. |
| `shell-edge` | 213 | Exact, 20 s. `echo` plus 71 `b`s is 76 bytes, runs, and prints 71 `b`s. Then 76 `a`s, `b`, two backspaces, and Enter echo `P` + 76 `a` + `\x08 \x08\x08 \x08` + newline, then `error: line too long (max 76)`. Backspace after overflow does not clear it. `exit` → `last=overflow`. |

After a shell step is chosen, the harness records `start` as the log length **before** the first key (the shell echoes while later keys are still in flight), sends the keys, and waits until all of these hold, except on the last step (that step is `exit` or `halt` and does not print another prompt):

1. For a `line` step, the bytes after `start` contain the typed text followed by `\n`. For a `keys` step, the log grew past `start` and contains a `\n` after `start`.
2. The log ends with `r3> `, and those four bytes are preceded by `\n` or by `\x1b[H`.
3. The log length is unchanged across two reads 50 ms apart.

The ready wait uses the same rule with `start = 0` and the echo `R3ATH3_SHELL_READY\nR3@TH3 shell. Type help for commands.`. Limits are unchanged: 5 s per step, status 124 on a miss. `xp` is not a prompt wait.

The harness self-test accepts `idle_at_prompt` on a buffer ending in `\nr3> ` and on one ending in `\x1b[Hr3> `, and rejects `\nr3> x`, a prompt that does not start a line, and a log missing the step echo. `keys_at_prompt` accepts `b"x\nr3> "` with `start=0` and rejects `start == len`, a buffer with no `\n` after `start`, a buffer ending in `\nr3> x`, a prompt that does not start a line, and a non-bytes value. It rejects a cooperative log with B1/A1 swapped; a preempt log with `preemptions=0`, with `hog_spins=0`, and with four worker lines; a sleep log in order M, S, L, with `elapsed=9 want=10`, and with `elapsed=200 want=10`. One positive log passes each of those three judges. A `sched-reap` log with one line removed, or with two lines swapped, fails. Every new case's correct log with a wrong status fails, including `sched-full` judged as 233 and `sched-reap` judged as 241. The `shell-ps` transcript judged against `shell-wait`, and `shell-usage` against `shell-sched-error`, both fail.

Totals: 56 QEMU cases, plus `harness-selftest` and 25 build lines. One clean-tree `./scripts/smoke.sh` is `SMOKE PASS`, `smoke_exit=0`, `dirty_entries=0` before and after.

## 11. Unsafe

`unsafe` stays in the driver files: `port.rs`, `serial.rs`, `vga.rs`, `qemu_exit.rs`, `irq.rs`, `exceptions.rs`, `memmap.rs`, `heap.rs`, `frames.rs`, the assembly in `boot.rs`, and `context.rs`. New blocks are the switch and trampoline, the stack pool, the canary, the initial frame, the saved-RSP table, `IrqCell`, `irq::without_interrupts`, and `vga::with_console`. The timer hook is a plain call and adds no `unsafe`. Every new block or impl has a `// SAFETY:` comment. `sched.rs`, `schedtest.rs`, `shell.rs`, `script.rs`, `mem.rs`, `memtest.rs`, and `main.rs` have none.

`vga::console_pos` is gone. `with_console` hands the cursor to a closure and the `&mut` cannot escape. Safety: one CPU; only the shell task calls the console; IRQ handlers and other tasks never do; a preemption mid-call is harmless because no other task enters the module. `console_put`, `console_backspace`, and `console_clear` call it. `shell-screen` is the behavior check.

`irq.rs` `KbData`: one CPU. IRQ1, the only producer, writes a slot at the tail only before publishing `KB_TAIL` with `Release`. The only consumer (the shell task, via `pop_key`) reads a slot at the head only after an `Acquire` of `KB_TAIL`, then publishes `KB_HEAD`. No slot is accessed by both at once.

## Decisions recorded in this spec

- 209 covers seven shell cases, 211 covers three, and 213 covers two. They stay shared outcome statuses. The scheduler's five proofs have their own statuses.
- Stacks are the static 112 KiB `.bss` pool, not the heap and not frames.
- An exited task keeps its slot until `wait`. An un-waited task, such as `hog` or a `spin` that nobody waits for, holds the slot.
- The quantum is one PIT tick. There are no priorities.
- The shell build runs the scheduler, with preemption on.
