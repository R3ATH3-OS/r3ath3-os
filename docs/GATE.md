# Human gate

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

Headline: the AI is a crew member with a badge. It can't touch the machine without a human's approval at the gate.

This document is the spec for the gate. If the code and this document disagree, the change is not done.

There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0, target `x86_64-unknown-none`.

The model never runs in the kernel. It stays on the host and reaches the kernel only by writing request lines on COM1. The gate's enforcement is inside the kernel. The shell task owns the gate value and is the only code that starts an action on an agent's behalf, and only from `Decision::Run` or `Approved::Go`.

Honest limits, on purpose:

- Agent names are self-asserted. The name in a request line is whatever the sender writes. Anyone who can write COM1 can claim any name. The gate still refuses a privileged action unless a human approves that exact request, and a name can ask only for what its badge allows.
- Revocation does not stop a task an earlier approval already started. There is no `kill`, and revocation does not undo output.
- Badges do not survive a reboot. There is no storage until milestone 10.
- There is no queue. One privileged request may wait. Another is `busy`.
- The flight recorder (`docs/RECORDER.md`) appends every gate outcome in `report`. The record is RAM-only and is lost on reboot until milestone 10.
- `next_rq` starts at 1 and uses wrapping addition. Wrapping is unreachable in a real session.

## 2. Scope

In this image:

1. A pure gate state machine in `kernel/src/gate.rs`: up to 4 badges, 4 verbs, default deny, grant and revoke, one pending privileged request, approve and deny, and expiry.
2. COM1 as the agent channel in the shell build. The shell reads lines and answers them.
3. Human commands `badges`, `grant`, `revoke`, `approve`, and `deny`.
4. Proofs `gate-core` (status 241) and `gate-expire` (status 243).

Not in this image, and not added here: any model or agent runtime in the kernel; a host-side AI adapter beyond the smoke harness; persistence of badges or of the flight recorder; authentication or crypto for names; a queue of pending requests; permissions on files, storage, network, or WASM; letting an agent type shell commands or reach `dispatch`; a `run` or `cmd` verb; an agent verb that reads the record; `kill`; COM2; unmasking IRQ4 or any IRQ besides 0 and 1; heap allocation in the gate or the shell; new `unsafe`. The default boot path is unchanged: boot marker, status 33. The flight recorder, `replay`, and `selftest` are specified in `docs/RECORDER.md`.

## 4. Permission model

An **agent** is something outside the kernel that sends request lines on COM1. In tests, that sender is the smoke harness.

A **badge** is the kernel's record for one name: the name plus a set of permissions. A human creates and changes badges at the keyboard. An agent cannot create, read, or change its badge.

A **permission** is one verb an agent may ask for. For a privileged verb it is necessary and not enough.

An **approval** is a human's yes for one specific privileged request, given at the keyboard by request number.

`read` and `say` need only the granted permission. `spawn` and `halt` also need approval of that exact request each time.

| Verb | Request form | Tier | What it does when allowed | Plain-words explanation |
| --- | --- | --- | --- | --- |
| `read` | `read` | free | Sends the 3 fixed work orders as `R3GATE data` lines. Read-only. | (none; the shell prints a notice) |
| `say` | `say TEXT` | free | Prints `<agent> says: TEXT`. TEXT is 1–40 bytes, each `0x20`–`0x7E`. | (none; the line itself is the notice) |
| `spawn` | `spawn spin` or `spawn nap` | privileged | `sched::spawn` of the existing shell demo, the same functions the `spawn` built-in uses. | `start demo task nap (sleeps 0.2 s, then exits)` or `start demo task spin (runs forever; no way to stop it)` |
| `halt` | `halt` | privileged | The shell halt path: `R3ATH3_SHELL_HALT`, status 217. | `stop the machine (ends this session)` |

Work orders are `WORK_ORDERS` in `gate.rs`:

1. `WO-1 check spindle oil level on mill 2`
2. `WO-2 replace air filter on compressor A`
3. `WO-3 log hours for weekly maintenance`

No verb can reach `dispatch`. An agent cannot run a shell command.

`MAX_BADGES = 4`. `Gate` holds `badges: [Option<Badge>; 4]`, `pending: Option<Pending>`, `next_rq: u32`, and `timeout: u64`. `Badge` is a `Name` plus a `u8` of bits: `read=1`, `say=2`, `spawn=4`, `halt=8`. A `Name` is 1–8 bytes, each `a`–`z` or `0`–`9`, stored as `[u8; 8]` plus a length.

The `Gate` value lives on the shell task's stack, created once as `Gate::new(6000)`. It is not a static. On this toolchain `size_of::<Gate>()` is 128 bytes, under the 200-byte bound for that stack. Badges die on reboot. `6000` ticks is 60 seconds at the nominal 100 Hz.

Default deny. No badge, or a badge without the verb's bit, is refused at once. A refusal is a one-line notice, not a question.

`grant A P` creates the badge when `A` is new and a slot is free, then sets bit `P`. Granting a bit already held is a success that changes nothing. A full table is an error and changes nothing.

`revoke A P` clears bit `P`. `revoke A all` removes the badge. Revoking a bit that is not held is a success that changes nothing. An unknown name is an error. A badge with no bits left still occupies its slot until `revoke A all`.

Revocation is immediate. In the same `revoke` call, if the pending request belongs to `A` and needs a permission `A` no longer holds, that request is cancelled and its denial is reported before the next prompt.

`approve N` re-checks the badge bit before executing. `Gate::force_clear_for_test` clears a bit without the cancel path and is compiled only for `gate-core`, which is how that re-check is reached. The shell build does not have it.

One pending request. A privileged request while another is pending is `busy`. Free verbs still run while one is pending.

Privileged requests are numbered `1, 2, 3, …` per boot. `approve` or `deny` with any other number is an error and changes nothing. A decided, cancelled, or expired number cannot be approved again.

A pending request expires when `now >= asked_at.saturating_add(timeout)` and is denied with `reason=timeout`. Only `Gate::poll(now)` notices expiry. The shell polls on every idle pass, before it takes a buffered agent line, and immediately before `approve` and `deny`. A late `approve` prints the expiry and then `error: no pending request <N>`.

## 5. The channel and the human

COM1 is the judged output and, in the shell build, the agent input. The kernel does not echo agent bytes. Agent bytes never enter the keyboard ring, the line editor, or `dispatch`.

Host to OS, one ASCII line, ended by `\n` (`\r` ignored), at most 72 bytes before the newline:

```
R3GATE ASK <agent> <seq> <verb> [arg]
```

Tokens are separated by exactly one space, with no leading or trailing space. `<agent>` is a valid `Name`. `<seq>` is 1–6 decimal digits and at least 1. `<verb>` is `read`, `say`, `spawn`, or `halt`. `read` and `halt` take no argument. `spawn` takes exactly `spin` or `nap`. `say` takes the rest of the line, inner spaces kept. A bad line replies with `verb=-`, and with `agent=` and `seq=` only when both parsed.

| Condition | Reason |
| --- | --- |
| Does not start with `R3GATE ASK `, or the name, seq, or token shape is bad, or a trailing space leaves the argument empty (`R3GATE ASK ai1 1 read `) | `syntax` |
| More than 72 bytes before the newline | `too-long` (the rest of the line is discarded, and agent and seq stay `-`) |
| Any other verb, including `approve` or `deny` | `verb` |
| Wrong argument | `arg` |

OS to host, serial only, never VGA, one reply per request line:

```
R3GATE reply agent=<a|-> seq=<n|-> verb=<v|-> result=<pending|done|denied|bad|failed>[ rq=<n>][ id=<task>][ reason=<word>]
```

For `read` only, before that reply:

```
R3GATE data seq=<n> n=<1..3> text=<work order>
```

A privileged request gets `result=pending rq=<n>` at once, then a second reply when it is decided: `done`, `denied reason=human|revoked|timeout`, or `failed reason=full` (approved, but the task table is full). Reasons are `no-badge`, `no-permission`, `busy`, `human`, `revoked`, `timeout`, `full`, `log-full`, `syntax`, `too-long`, `verb`, and `arg`. `reason=log-full` is the flight recorder's reserve refusal: the shell prints `gate: ignored an agent line: flight recorder full`, does not call `gate.ask`, and records nothing. See `docs/RECORDER.md`.

The human answers by typing `approve N` or `deny N`, naming the request number. A modal yes/no key is not used, so a key meant for something else cannot approve a request that just arrived.

The shell loop reads a key first. It acts on expiry, then on an agent line, only while the human's line buffer is empty, so a notice does not break into a line the human is typing and a run of agent lines cannot postpone the timeout reply. While the human types, the agent line stays buffered and is handled after Enter. At an idle prompt an agent event prints a newline first (the cursor sits after `r3> `), then the human lines on serial and VGA, then the `R3GATE` lines on serial only, then `r3> `. Agent events do not change `last`. An approved spawn that finds the task table full sets `last` to cmd. When the shell is blocked in `wait`, agent lines and expiry wait until `wait` returns. `wait` on `spin` still blocks forever.

| Event | Lines |
| --- | --- |
| Privileged request accepted | `gate: request <rq> from <agent> wants to:` / `gate:   <explanation>` / `gate: type approve <rq> or deny <rq> within 60 s` |
| Refused | `gate: refused <agent>: no badge` · `gate: refused <agent>: no permission to <verb>` · `gate: refused <agent>: another request is waiting` |
| Bad line | `gate: ignored a bad request line (<reason>)` |
| `read` ran | `gate: <agent> read the work orders` |
| `say` ran | `<agent> says: <text>` |
| Approved spawn | `gate: request <rq> approved: started task <id> (<demo>)` |
| Approved, table full | `gate: request <rq> approved, but the task table is full` |
| Approved halt | `gate: request <rq> approved: stopping the machine` |
| Denied | `gate: request <rq> denied` |
| Cancelled, or the re-check failed | `gate: request <rq> cancelled: <agent> may no longer <verb>` |
| Expired | `gate: request <rq> expired after 60 s: denied` |

Human lines are at most 80 columns. `say` text is printed after `<agent> says: `, so it never starts a line. Every judge matches whole lines.

Order for each event: human lines, then `R3GATE data` lines for `read`, then the `R3GATE reply` line. An approved halt prints the human line, then `R3GATE reply … result=done`, then `R3ATH3_SHELL_HALT` and status 217.

`fn report(out: &mut Out, ev: &GateEvent)` prints every gate outcome and appends it to the flight recorder. `GateEvent` is `Asked`, `Refused`, `Bad`, `Ran`, `Approved`, `ApproveFailed`, `Denied`, `Cancelled`, `Expired`, `Granted`, or `Revoked`.

## 6. Shell commands

`help` is 19 lines. These five sit after `wait ID` and before `log`. The three recorder lines sit after `deny N` (`docs/RECORDER.md`, `docs/SHELL.md`):

```
badges      list AI badges and the waiting request
grant A P   give agent A permission P
revoke A P  take permission P (or all) from agent A
approve N   let gate request N run
deny N      refuse gate request N
```

| Command | Success (`last` = ok) | Errors (`last` = cmd) |
| --- | --- | --- |
| `badges` | One `badge <agent> may=<p1,p2,…\|none>` line per badge in slot order. Bits print in the order read, say, spawn, halt. No badges: `no badges`. Then `pending rq=<n> agent=<a> action=<spawn spin\|spawn nap\|halt>` or `no pending request`. | Any argument: `error: usage: badges` |
| `grant A P` | `granted <A> <P>` | Not exactly two words: `error: usage: grant AGENT PERM`. Bad name: `error: bad agent name: <A>`. Unknown verb: `error: unknown permission: <P>`. New name and 4 badges exist: `error: badge table full`. The name is checked before the permission. |
| `revoke A P` / `revoke A all` | `revoked <A> <P\|all>`, then any cancellation line and its `R3GATE reply` | Not exactly two words: `error: usage: revoke AGENT PERM`. Bad name: `error: bad agent name: <A>`. Unknown verb other than `all`: `error: unknown permission: <P>`. No such badge: `error: no badge <A>`. |
| `approve N` | The approved line and the `R3GATE reply` | Not exactly one word: `error: usage: approve N`. A full flight recorder, before the number is parsed: `error: flight recorder full: approve refused`. Not a number, or not the pending number: `error: no pending request <N>`. |
| `deny N` | `gate: request <n> denied` and the `R3GATE reply` | `error: usage: deny N`. `error: no pending request <N>`. |

No shell path allocates. `shell-mem` keeps `heap_used <= 4096`.

## 7. Proofs

All gate lines are `R3ATH3_GATE_*`. A failed internal check panics (status 35), which no case accepts.

`gate-core` uses `Gate::new(100)` and a fake clock. It prints `R3ATH3_GATE_CORE step=<k> <text>` for these 14 steps, then `R3ATH3_GATE_CORE ok steps=14`, and exits 241.

1. `ask ai1 read` with no badge → `step=1 refused=no-badge`
2. grant `ai1 read`; `ask read` → `step=2 run=read`
3. `ask spawn nap` → `step=3 refused=no-permission`
4. grant `ai1 spawn`; `ask spawn nap` at now=0 → `step=4 pending rq=1`
5. `ask spawn spin` → `step=5 refused=busy`
6. `approve 2` is `NoPending`; `approve 1` is `Go` → `step=6 wrong=no-pending approved rq=1 action=spawn-nap`
7. `approve 1` again → `step=7 replay=no-pending`
8. `ask spawn nap` at now=10 is rq=2; `deny 2` → `step=8 denied rq=2`
9. `ask spawn nap` at now=20 is rq=3; `revoke ai1 spawn` returns that request; `approve 3` is `NoPending`; the next `ask spawn nap` is refused → `step=9 cancelled rq=3 then=no-pending next=no-permission`
10. `ask read` → `step=10 run=read`
11. grant `ai1 halt`; `ask halt` at now=30 is rq=4 with deadline 130; `poll(129)` is none; `poll(130)` expires it; `approve 4` is `NoPending` → `step=11 expired rq=4 at=130`
12. grant `ai1 spawn`; `ask spawn nap` at now=200 is rq=5; `force_clear_for_test` clears spawn without cancelling; `approve 5` is `Cancelled` → `step=12 recheck=cancelled rq=5`
13. `revoke ai1 all`; `ask read` is `no-badge`; grant `a`, `b`, `c`, and `d` `read`; grant `e read` is `BadgeTableFull` → `step=13 removed refused=no-badge full=e`
14. `parse_request` on the seven fixed lines prints `step=14 parse=ok,syntax,too-long,verb,arg,syntax,syntax`

The seven lines are `R3GATE ASK ai1 1 read`, `halt`, `R3GATE ASK ai1 2 say ` plus 60 `x` bytes, `R3GATE ASK ai1 3 approve 1`, `R3GATE ASK ai1 4 spawn bomb`, `R3GATE ASK AI1 5 read`, and `R3GATE ASK ai1 1 read ` (the trailing space is `syntax`, and the agent and seq are parsed).

`gate-expire` runs with preemption on and the real PIT. `Gate::new(50)`. Grant `ai1 spawn`. Ask `spawn nap` at `t0 = irq::ticks()`. Sleep 5 ticks and poll until the request expires. Print `R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=<irq::ticks()-t0> early_polls=<n>` and exit 243. The test does not execute the action.

## 9. Statuses

| Port value | Status | Meaning | Helper |
| --- | --- | --- | --- |
| `0x75` | 235 | `sched-reap` ok | `sched_reap_ok` |
| `0x78` | 241 | `gate-core` ok | `gate_core_ok` |
| `0x79` | 243 | `gate-expire` ok | `gate_expire_ok` |

`0x76`–`0x77` (237–239) and `0x7B`–`0x7F` (247–255) stay unused. `0x7A` (245) is `recorder-core` (`docs/RECORDER.md`). Each helper is compiled only for its feature. Gate shell cases use the shell outcome statuses 209, 211, and 217.

## 10. Determinism

`gate-core` uses a fake clock, so every line is exact. `gate-expire` is judged by bounds only: `waited >= 50`, `waited <= 150`, and `early_polls >= 1`. No judge compares an exact tick. Gate shell cases are exact transcripts. The harness sends an agent line only when the shell is idle at an anchored prompt, and the shell acts on an agent line only with an empty line buffer. No shell case waits for the 60 second expiry. A gate that never answers is a timeout (124).

## 11. Smoke

`expect_gate` is `expect_shell` with a serial socket and an `ai` step. Serial is `-chardev socket,id=com1,path=<OUT>/<name>.ser.sock,server=on,wait=on -serial chardev:com1`. `wait=on` holds the guest until the harness connects, so the boot lines are not lost. The harness connects to the serial socket first, then the monitor socket, reads serial into a buffer, and writes that buffer to `<name>.serial.log` on exit and on every failure path. An `ai <line>` step writes `<line>\n` in 8-byte chunks 1 ms apart, then waits until the bytes after the snapshot contain `R3GATE reply `, the buffer ends at an anchored `r3> `, and the length is unchanged across two reads 50 ms apart. The limit is 5 seconds. `line` and `keys` steps keep the `expect_shell` rules.

| Case | Status | What it checks |
| --- | --- | --- |
| `gate-core` | 241 | The 14 step lines and `ok steps=14`, exact |
| `gate-expire` | 243 | One expire line, `50 <= waited <= 150`, `early_polls >= 1` |
| `gate-approve` | 209 | `spawn nap` does nothing while pending (`ps` shows ids 0 and 1), then runs once after `approve 1`, and `wait 2` reaps it |
| `gate-noperm` | 209 | No badge, then no spawn permission, then `read` returns the three work orders |
| `gate-revoke` | 209 | Revoking `spawn` cancels the pending request in that command; `say` still works; `revoke all` removes the badge |
| `gate-sneak` | 211 | Bad lines, `busy`, a denied `spin` that never runs, and a number that cannot be approved again |
| `gate-halt` | 217 | Approve of `halt` prints the reply, then `R3ATH3_SHELL_HALT` |
| `gate-usage` | 211 | Usage, bad names, unknown permissions, and a full badge table. 20 seconds |

The exact `gate-approve` transcript is the one in the smoke harness. The other five gate transcripts are exact too.

## Try it by hand

Build the shell and run QEMU with a display and `-serial stdio`. Type human commands in the QEMU window. Type `R3GATE ASK …` lines in the terminal, as the agent:

```sh
cd kernel && cargo build --release --features shell
qemu-system-x86_64 -machine pc,accel=tcg -cpu qemu64 -m 64M -smp 1 \
  -kernel target/x86_64-unknown-none/release/r3ath3 \
  -nic none -serial stdio -no-reboot \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04
```
