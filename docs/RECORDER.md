# Flight recorder

Suggested wording, not a certification and not a claim that the system exists yet: "An authentic Human–AI co-op OS".

This document is the spec for the flight recorder and `selftest`. If the code and this document disagree, the change is not done.

There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0, target `x86_64-unknown-none`.

The record is the first one the kernel keeps. Milestone 8 printed gate outcomes. This image appends them, and writes each entry through to the store. Without an NVMe disk the store is the RAM disk, and a reboot erases both. With QEMU's emulated NVMe disk, earlier boots stay in the host image and are read with `boots K`. The live record still starts empty every boot (`docs/STORAGE.md`).

## 1. Scope

In this image:

1. `recorder.rs`: a pure, safe, generic, fixed-capacity append-only record with a hash chain, and the body formatter.
2. Recording in the shell. Every gate event goes through `report()`. Every human command line is recorded with its outcome. One summary entry is recorded per `selftest`.
3. The full-record policy. No entry is overwritten. Agent lines are refused when only the reserve is left. `approve` is refused when the record is full. The human is never locked out, and unrecorded actions are counted.
4. Human commands `log`, `replay`, `replay N`, and `selftest`. `help` is 24 lines.
5. `selftest.rs`: eight safe on-machine checks and the report card. The eighth, `store`, is specified in `docs/STORAGE.md`.
6. One kernel proof, `recorder-core` (status 245, fake clock).
7. Four shell cases (`log-record`, `log-full`, `log-usage`, `shell-selftest`) and the `log` judge.

Not in this image, and not added here: a file system; WASM, a framebuffer, or sound; any model, inference, or agent runtime in the kernel; crypto (SHA-256 was considered in milestone 10 and not adopted, see `docs/STORAGE.md`; also HMAC, keys, or signatures); an agent verb or any COM1 path that reads, clears, or changes the record; clearing, rotating, or overwriting the record; re-executing recorded actions; undoing approved actions; queueing more than one pending request; recording keystrokes, raw agent bytes, `R3GATE data` lines, or scheduler internals; wall-clock dates or reading the RTC in the shell; fault injection, panics, or spawned tasks inside `selftest`; changes to the gate's decisions, verbs, or the reply protocol, except the reason word `log-full`; heap allocation in the shell, recorder, or selftest; new `unsafe`. The default boot path is unchanged: boot marker, status 33.

Honest limits, on purpose:

- The live record is written through to the store. Without an NVMe disk that store is the RAM disk, and a reboot erases both. With QEMU's emulated NVMe disk, earlier boots are read with `boots K`. The live record starts empty every boot.
- In this emulator build, COM1 TX carries the human transcript, including `log` and `replay`. Whoever reads the host side of COM1 sees what the human sees. No request can fetch the record, and nothing on COM1 can change it. The same lines also print on VGA, which does not pass through the host's serial capture.
- Agent names are self-asserted. The record stores the claimed name.
- The chain is not crypto. It is not keyed and not collision-resistant.
- `exit` and `halt` are not recorded. They stop the machine, and the RAM record goes with them. Their outcome is the QEMU status. An approved agent `halt` is recorded, because `report` runs before the halt, and the machine stops right after.

## 2. Where it lives

`kernel/src/recorder.rs` is safe code only: `Entry`, `Recorder<const N: usize>`, `link()`, `Verify`, `AppendError`, `Body`, and `event_body()`. No I/O, no statics, no allocation, no `unsafe`. Time comes in as a `tick: u64` argument.

`kernel/src/recordertest.rs` is the `recorder-core` body. `kernel/src/selftest.rs` holds the check names, `Card`, and the report-card line formatters (compiled for `any(shell, recorder-core)`). The eight checks are compiled only for `shell`. `selftest.rs` never sees the live gate, the live record, or the store. The shell passes the store result in.

The live record is a module-private `static LOG: IrqCell<Recorder<128>>` in `shell.rs`. `IrqCell` is the existing safe wrapper the scheduler table already uses, so this adds no `unsafe`. Module privacy keeps any other kernel module from naming it. `Recorder::new()` is a `const fn` whose bytes are all zero, and the empty-record head is computed rather than stored, so the static lands in `.bss`.

The gate (128 bytes) lives on the shell stack. `Recorder<128>` is 16400 bytes. The shell runs on the 16 KiB boot stack, which has no canary and no guard page, so the record cannot go there.

Two rules for `LOG.with`:

1. Never call `LOG.with` inside another `LOG.with`.
2. Never print inside the closure. Copy entries out with `get()` and print after the closure returns.

`recorder` is compiled only with `feature = "recorder"`. `recorder = []` is internal. `recorder-core = ["recorder"]`. `shell = ["sched", "gate", "recorder"]`. Every build without `recorder` behaves as it did before this milestone. `recorder-core` joins the "at most one" assert and both `boot_ok` lists.

## 3. Words

- **Entry:** one recorded action: a number, a tick, a tag, a body, and a link.
- **Record:** the fixed array of entries since boot, in order. It is append-only: nothing is overwritten, removed, or edited.
- **Link:** the hash-chain value after an entry. **Head:** the link of the newest entry, or the FNV basis when the record is empty.
- **Lost:** a count of actions that happened but could not be recorded.

## 4. What is recorded

| Source | When | Tag | Body |
| --- | --- | --- | --- |
| Human command | After `dispatch` returns for a non-empty line, and after the overflow error for an overflowed line | `human last=<ok\|cmd\|overflow\|script>`: the value of `last` that this line set | The trimmed line as typed, 0–76 bytes |
| Agent line | In `report()` for `Asked`, `Refused`, `Bad`, `Ran` | `agent <name>`, or `agent -` when no name parsed | `event_body` without the agent |
| Gate decision | In `report()` for `Approved`, `ApproveFailed`, `Denied`, `Cancelled`, `Expired` | `gate` | `event_body` with the agent |
| Badge change | In `report()` for `Granted`, `Revoked` | `gate` | `grant <A> <P>` or `revoke <A> <P\|all>` |
| Selftest | Once per `selftest`, after the card prints | `selftest` | `ok=<n> fail=<m>` |

Inside one human command, the gate entries come first, written as the gate acts. The human-line entry comes last, written when the command finishes and its outcome is known. `revoke ai1 spawn` with a pending request writes `gate` / `revoke ai1 spawn`, then `gate` / the revoked denial, then `human last=ok` / `revoke ai1 spawn`.

Not recorded:

- Empty lines, which are not actions.
- `exit` and `halt`, as above.
- Individual keystrokes and backspaces.
- Raw agent bytes. A bad line records only its reason and any parsed name and seq, never its bytes.
- `R3GATE data` lines. They carry fixed text, and the `read` entry stands for them.
- Scheduler switches and timer ticks.

Every failed `approve 9`, `grant e read` on a full badge table, and `revoke zz read` is recorded as a `human last=cmd` entry with the exact text typed. `report()` is not widened. The gate's own decisions still come only from `report()`.

## 5. Entry format

```rust
pub const TAG_MAX: usize = 24;
pub const BODY_MAX: usize = 76;
pub struct Entry {
    n: u32,
    tick: u64,
    link: u64,
    tag: [u8; TAG_MAX],
    tag_len: u8,
    body: [u8; BODY_MAX],
    body_len: u8,
}
```

`size_of::<Entry>()` is 128. `size_of::<Recorder<128>>()` is 16400.

Validity rules:

- Tag: 1–24 bytes. Body: 0–76 bytes.
- Every byte is printable ASCII, `0x20`–`0x7E`.
- `n` is 1-based and counts stored entries only, so `n` never exceeds the capacity and is never reused.

`event_body` builds one canonical body:

```
[<agent> ]<action> seq=<s|-> [rq=<n> ]<result>[ id=<task>][ reason=<word>][ text=<say text>]
```

- `action` is one of `read`, `say`, `spawn-nap`, `spawn-spin`, `halt`, or `-` (a bad line). `fn action_word` maps `Spawn(Nap)` to `spawn-nap`, and so on.
- `result` uses the same words as the reply: `pending`, `done`, `denied`, `bad`, `failed`.
- `reason` uses the reply reason words, plus `log-full`.

Examples:

```
spawn-nap seq=4 rq=1 pending
read seq=3 denied reason=no-badge
- seq=2 bad reason=verb
say seq=1 done text=hello
ai1 spawn-nap seq=4 rq=1 done id=2
ai1 spawn-spin seq=5 rq=2 denied reason=human
ai1 spawn-nap seq=6 rq=3 denied reason=revoked
```

`Expired` gives `ai1 halt seq=1 rq=4 denied reason=timeout`. `ApproveFailed` gives `ai1 spawn-spin seq=7 rq=5 failed reason=full`. No shell case reaches those two events. `recorder-core` step 11 proves both bodies byte for byte.

`log` and `replay` print every entry as two lines, both at most 80 columns:

```
#<n> t=<tick> <tag> link=<16 lowercase hex>
  <body>
```

The header is at most 74 columns, even with a 20-digit tick. The body line is two spaces plus at most 76 bytes. A body line always starts with two spaces, and a header always starts with `#`. Nothing an agent or a human types can start a header line. `say` text and human text always sit inside an indented body line.

## 6. Capacity and the full-record policy

`LOG_CAP = 128`. `AGENT_RESERVE = 8`.

Never overwrite. When the record is full, `append` stores nothing, returns `Err(Full)`, and adds one to `lost` (`u32`, saturating). A bad input also stores nothing and adds one to `lost`. The shell never produces bad input.

An agent line is acted on only while `free() > AGENT_RESERVE`. Otherwise the shell does not call `gate.ask`. The line is refused unrecorded and counted in `lost`:

- The shell still runs `parse_request`, which is pure, to fill in the reply fields.
- Human line: `gate: ignored an agent line: flight recorder full`.
- Reply: `R3GATE reply agent=<a|-> seq=<s|-> verb=<v|-> result=denied reason=log-full`. The agent, seq, and verb are filled when the line parses. Otherwise they follow the bad-line rule, and a too-long line gives all `-`.

The reserve keeps the last 8 entries for the human: deciding the one pending request, revoking, and reading the record. Without it, an agent could flood refused or bad lines until the record is full, then act without a trace.

`approve` is refused when the record is full: `error: flight recorder full: approve refused`, and `last` becomes cmd. No privileged agent action runs without its entry. Order inside `approve_cmd`: usage, then `note_expiry`, then the full check, then the number. `deny` is always allowed. It makes nothing run.

Humans are never locked out. Every other human command still works when the record is full. Its entry is counted in `lost`.

When an append fills the last slot, the shell prints, on both channels, exactly once per boot, right after that append and outside `LOG.with`:

```
log: flight recorder full; later entries are counted as lost
```

No command clears the record.

No agent action, free or privileged, runs without its entry stored. Admission guarantees room for the agent-line entry, and the `approve` check guarantees room for the `Approved` entry. The shell is the only writer and is single-threaded, so neither check can go stale before its append.

## 7. Integrity

The chain is 64-bit FNV-1a.

- `link_0 = 0xcbf29ce484222325` (the FNV-1a offset basis).
- `link_n` starts from `link_{n-1}` and runs FNV-1a (prime `0x100000001b3`, wrapping multiply) over the canonical bytes of entry `n`:

```
n as 4 bytes little-endian ‖ tick as 8 bytes little-endian ‖ tag_len (1 byte) ‖ tag ‖ body_len (1 byte) ‖ body
```

Reference:

```python
BASIS = 0xcbf29ce484222325; PRIME = 0x100000001b3; M = (1 << 64) - 1
def link(prev, n, tick, tag, body):
    h = prev
    for b in n.to_bytes(4, "little") + tick.to_bytes(8, "little") + bytes([len(tag)]) + tag + bytes([len(body)]) + body:
        h = ((h ^ b) * PRIME) & M
    return h
```

The standard FNV-1a-64 of `b"a"` from the basis is `af63dc4c8601ec8c`.

`verify()` recomputes every link from the basis and compares it with the stored link. It returns the first entry that does not match.

What it detects:

- Any change to a stored entry's `n`, tick, tag, or body.
- An entry inserted, removed, or reordered in the middle.
- A stored link that was not recomputed.

That covers accidental corruption (a stray write, a kernel bug) and naive edits. A human can note a `head=` value at the console. If the record is later truncated back past that point, or rewritten by something that is not trying to forge a match, a later `replay` disagrees.

What it does not prove:

- It is not keyed and not collision-resistant. Kernel code that can write this memory can rewrite entries and recompute every later link.
- Truncating the newest entries and lowering `len` is invisible unless the human noted the head.
- It proves nothing about a host that can edit the image. FNV-1a is not crypto. Without an NVMe disk the live record is written through to the RAM disk, and a reboot erases both. With the emulated disk, earlier boots are read with `boots K`.
- The kernel has no entropy source or key storage, and anyone who controls ring 0 also controls the verifier. That is why there is no crypto.

The agent has no write path into kernel memory. It can only send bounded, printable COM1 lines. The reserve covers floods, and the indented body line covers display forgery. The host can drop or edit serial output. The record stays in kernel RAM, and `log` and `replay` also print on VGA.

## 8. Who writes and who reads

The shell task is the only writer, through one helper, `record(out, tag, body)` in `shell.rs`. It samples `irq::ticks()` before the closure, appends inside `LOG.with`, and prints the full notice after the closure if that append filled the record. `record()` is called from:

- Each of the 11 `report()` arms, after that arm's lines print.
- The human-line hook in `run()`.
- `selftest_cmd`.

The refused-agent path calls `note_lost` only. Nothing else calls `append`. `selftest.rs` and `recordertest.rs` use only their own local `Recorder` values.

Each `report()` arm prints exactly what milestone 8 prints, then records the row in section 4. The two branches that print nothing (`Ran` with `Spawn` or `Halt`, and `Approved` for a spawn with `id: None`) are unreachable and record nothing. The expiry poll in the idle loop is unchanged. Its `report(Expired)` records like any other arm.

On Enter:

- Overflow: print `error: line too long (max 76)`, set `last` to overflow, then record the trimmed buffer with that `last`.
- A non-empty line: `dispatch`, then record the trimmed line with the `last` that dispatch set.
- `record_human` builds the tag `human last=<word>` with the same words as `leave()`, through `last_word`.

`handle_agent_line` checks `admits_agent(AGENT_RESERVE)` first. If that is false, it takes the refused path above and returns. If it is true, behavior is unchanged from milestone 8.

Readers are the human only, through `log`, `replay`, and `replay N`. `selftest` verifies the live chain and does not print it. No agent verb reads the record. `parse_request` refuses any verb other than `read`, `say`, `spawn`, and `halt` with `reason=verb`. `log-record` sends `R3GATE ASK ai1 2 log` to prove it.

A `Body` that overflowed, or a tag or body that breaks section 5, is counted as lost and is not stored. The formats in this spec cannot overflow, and `corrupt_for_test` does not exist in the shell build.

## 9. `recorder.rs` API

```rust
pub const TAG_MAX: usize = 24;
pub const BODY_MAX: usize = 76;
pub const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
pub fn link(prev: u64, n: u32, tick: u64, tag: &[u8], body: &[u8]) -> u64;
pub struct Entry { /* section 5 */ }
impl Entry {
    pub const EMPTY: Entry;
    pub fn n(&self) -> u32;
    pub fn tick(&self) -> u64;
    pub fn link(&self) -> u64;
    pub fn tag(&self) -> &[u8];
    pub fn body(&self) -> &[u8];
    pub fn from_parts(n: u32, tick: u64, link: u64, tag: &[u8], body: &[u8]) -> Option<Entry>;
}
pub enum AppendError { Full, Bad }   // both add one to lost
pub enum Verify { Ok, Broken { at: u32 } }
pub struct Recorder<const N: usize> { entries: [Entry; N], len: usize, lost: u32 }
impl<const N: usize> Recorder<N> {
    pub const fn new() -> Self;       // all-zero; lands in .bss
    pub fn append(&mut self, tick: u64, tag: &[u8], body: &[u8]) -> Result<u32, AppendError>;
    pub fn note_lost(&mut self);
    pub fn len(&self) -> usize;
    pub fn capacity(&self) -> usize;
    pub fn free(&self) -> usize;
    pub fn is_full(&self) -> bool;
    pub fn lost(&self) -> u32;
    pub fn head(&self) -> u64;        // BASIS when empty
    pub fn admits_agent(&self, reserve: usize) -> bool; // free() > reserve
    pub fn get(&self, n: u32) -> Option<Entry>;         // 1..=len
    pub fn verify(&self) -> Verify;
    pub fn verify_upto(&self, n: u32) -> Verify;
}
pub struct Body { /* [u8; BODY_MAX] + len; as_bytes(); overflow() if a write did not fit */ }
pub fn event_body(
    agent: Option<&[u8]>, action: &str, seq: Option<u32>, rq: Option<u32>,
    result: &str, id: Option<usize>, reason: Option<&str>, text: Option<&[u8]>,
) -> Body;
```

`#[cfg(feature = "recorder-core")] fn corrupt_for_test(&mut self, n: u32)` flips bit 0 of body byte 0 of entry `n`. It is absent from the shell build.

## 10. Commands

Replay walks the record in order, verifies each step's link, and prints it. `replay` walks all of it. `replay N` shows one step. It never re-executes anything. Re-running `spawn` or `halt` from the record would itself be an unapproved privileged action, and the machine's state now is not its state then.

These three help lines sit after `deny N` and before `run SCRIPT`:

```
log         newest flight-recorder entries and totals
replay [N]  check the record and print it, or entry N
selftest    run the feature checks and print a report card
```

Every line is at most 80 columns and goes to both channels.

| Command | Success output (`last` = ok) | Errors (`last` = cmd) |
| --- | --- | --- |
| `log` | `log entries=<len> of 128 lost=<lost> head=<16 hex>`, then the newest `min(5, len)` entries, oldest first, two lines each | Any argument: `error: usage: log` |
| `replay` | Every entry `#1`…`#len` in order, two lines each, then `replay ok entries=<len> lost=<lost> head=<16 hex>` | Broken chain at entry `k`: entries `1`…`k-1`, then `replay: chain broken at #<k>`. More than one argument: `error: usage: replay [N]` |
| `replay N` | Entry `N` (two lines), then `replay step <N> of <len> ok`. Entries `1`…`N` are verified first. | Not a number, `0`, or more than `len`: `error: no log entry <N>`, using the raw argument. Broken chain at `k <= N`: `replay: chain broken at #<k>` and no earlier entries |
| `selftest` | The report card in section 11. `last` is ok when every check passes | Any failed check: the same card, `last` = cmd. Any argument: `error: usage: selftest` |
| `approve N` | Unchanged from `docs/GATE.md` | A full record: `error: flight recorder full: approve refused`, before the number is parsed |

Each command's own human-line entry is appended after its output. `log` and `replay` therefore never show their own entry, and the next `log` does.

No shell path allocates. `LOG.with` closures copy one `Entry` (128 bytes) out at a time. `shell-mem` keeps `heap_used <= 4096`.

## 11. `selftest`

The checks are safe, bounded, and read-mostly. A normal `selftest` never panics, never takes a fault path, never spawns a task, never touches the live gate, and never changes the live record except for its own summary entry. Each check returns `Result<(), &'static str>`. All eight run, then the card prints. There is no `unwrap` and no `panic!` in `selftest.rs`. `selftest.rs` never calls `sched::spawn`, `qemu_exit`, or anything in `shell.rs`. The `store` check is computed by the shell with `store::check_live` before `selftest::run`. It only reads. Its failure details are `superblock unreadable`, `store write failed`, and `record copy differs` (`docs/STORAGE.md`).

| # | Name | What it does | Failure detail |
| --- | --- | --- | --- |
| 1 | `timer` | `t0 = irq::ticks()`. Loop up to `TIMER_SPIN_LIMIT = 2_000_000` times: stop when `irq::ticks() >= t0 + 5`, else `sched::yield_now()`. A dead timer reports a failure instead of hanging. | `ticks did not advance` |
| 2 | `sched` | Needs `timer` to have passed. `s0 = sched::switches()`, `sched::sleep_ticks(2)`, require `switches() > s0`. `snapshot()` slot 0 is `shell` and `running`, and slot 1 is named `idle`. | `needs the timer` · `no task switch while sleeping` · `task table does not show shell and idle` |
| 3 | `frames` | `(t, f) = frames::stats()`. Require `f > 0`. `alloc()` returns `Some`, and `stats().1 == f - 1`. `free(frame)` returns `Ok`. `stats() == (t, f)`. This zeroes one free frame and returns it. The frame is still freed if the drop check fails. | `no free frame` · `free count did not drop by one` · `free returned an error` · `counts not restored` |
| 4 | `script` | `run_inline(b"set n 2; add n 3; print n; say \"ok\"", &mut cap)` into a fixed-buffer capture sink. Require `Ok` and exactly the lines `n=5`, `ok`. | `script error` · `script output differs` |
| 5 | `gate` | A scratch `Gate::new(100)` with a fake clock and agent name `st`. Expect, in order: read with no badge → `Refused(NoBadge)`. Grant read; read → `Run(Read)`. `spawn nap` → `Refused(NoPermission(Spawn))`. Grant spawn; `spawn nap` at 0 → `Pending { rq: 1 }`. `spawn spin` → `Refused(Busy)`. `approve(2)` → `Err`. `approve(1)` → `Go` with `rq == 1`; the returned `Pending` is dropped and nothing is executed. `approve(1)` → `Err`. `spawn nap` at 10 → `Pending { rq: 2 }`. `poll(109)` → `None`. `poll(110)` → expired rq 2. | `gate decision differs` |
| 6 | `recorder` | The shell passes `live_ok = LOG.verify() == Ok`. Then a scratch `Recorder<4>`: four appends (ticks 1–4, tag `selftest`, body `x`) give `Ok(1..=4)`, the fifth gives `Err(Full)`, `lost() == 1`, `verify() == Ok`, and `head()` equals `get(4)`'s link. | `live chain broken` · `scratch log misbehaved` |
| 7 | `store` | The shell's `check_live`: the superblock reads, this boot's record matches the live chain, and a failed write-through is reported. When this boot has no record, the check passes. | `superblock unreadable` · `store write failed` · `record copy differs` |
| 8 | `heap` | `heap::used()` read at the start of `selftest` equals `heap::used()` read now, and is at most `heap::SIZE`. | `heap use changed during selftest` |

The card is exact. Names are padded to 8 columns, then one space:

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

A failed check prints `selftest <name padded to 8> FAIL <detail>`, for example `selftest frames   FAIL no free frame`. The summary becomes `selftest: <ok> of 8 ok, <fail> failed`.

After the card, the shell records `selftest` / `ok=<n> fail=<m>`. That append happens before the human-line entry. `selftest` takes at least 70 ms (5 + 2 ticks). Agent lines wait in the buffer meanwhile, as they do during `wait`.

The failure lines are proven byte for byte by `recorder-core` steps 9–10 through the same formatters. The all-pass card is proven by `shell-selftest`.

## 12. Kernel proof `recorder-core`

No scheduler and no timer: every tick is a literal. Each step prints one line `R3ATH3_RECORDER_CORE step=<k> …`, with the exact texts below. Then the proof prints `R3ATH3_RECORDER_CORE ok steps=11` and exits 245. Any internal check that fails panics (status 35), which no case accepts.

`r = Recorder::<6>::new()`, `RES = 2`.

1. Empty: `step=1 len=0 lost=0 head=cbf29ce484222325 verify=ok`
2. `append(10, b"human last=ok", b"help")` → `Ok(1)`: `step=2 n=1 link=ebb95e48118d0282`
3. `append(20, b"agent ai1", b"say seq=1 done text=hi")` → `Ok(2)`, verify ok: `step=3 n=2 link=4d9b4cc92b89320a verify=ok`
4. `append(30, b"gate", b"grant ai1 say")` → 3. `admits_agent(RES)` → true. `append(40, b"gate", b"revoke ai1 all")` → 4. `admits_agent(RES)` → false: `step=4 len=4 agent_before=yes agent_after=no`
5. `append(50, b"human last=cmd", b"approve 9")` → 5. `append(60, b"selftest", b"ok=7 fail=0")` → 6, and the record is full. `append(70, b"human last=ok", b"log")` → `Err(Full)`, `lost() == 1`, head unchanged: `step=5 len=6 full=yes lost=1 head=68712f5597559a1a`
6. A fresh `Recorder::<6>`. Each of these returns `Err(Bad)`: a 25-byte tag; a 77-byte body; body `tab\there` (contains `0x09`); an empty tag. Then `note_lost()`. Result: `len == 0`, `lost == 5`: `step=6 bad=4 noted=1 len=0 lost=5`
7. `corrupt_for_test(3)`. `verify()` → `Broken { at: 3 }`. `verify_upto(2)` → `Ok`: `step=7 tamper=3 verify=broken at=3 upto2=ok`
8. `get(0)` → `None`. `get(7)` → `None`. `get(1)` → tick 10, tag `human last=ok`, body `help`: `step=8 get0=none get7=none get1=10,human last=ok,help`
9. The check-line formatter for frames gives `step=9 card=selftest frames   FAIL no free frame`
10. The summary formatter for that card gives `step=10 card=selftest: 7 of 8 ok, 1 failed`
11. `event_body` for the expired halt and the failed spawn: `step=11 expired=ai1 halt seq=1 rq=4 denied reason=timeout|failed=ai1 spawn-spin seq=7 rq=5 failed reason=full`

The links in steps 2, 3, and 5 come from section 7. Links 3–5 of that record are `7e04d0e40086d407`, `36286de24c5c145c`, and `41b02d0be281909e`.

## 13. Exit status

| Port value | Status | Meaning | Helper |
| --- | --- | --- | --- |
| `0x7A` | 245 | `recorder-core` ok | `recorder_core_ok` (compiled only for `recorder-core`) |

Shell cases use the shell outcome statuses 209 and 211. The store proof uses 247. The NVMe proof uses 249. The unused odd statuses are 203, 205, 207, 219, 221, 223, 237, 239, 251, 253, and 255: 11 values.

## 14. Determinism

- `recorder-core` uses a fake clock, so every line is exact, including the hash values.
- `shell-selftest` and `log-usage` print no ticks and no hashes. They are exact transcripts under `judge_exact`. The selftest checks are deterministic by construction: `sleep_ticks` returns only after its deadline, and the shell's sleep always causes a switch.
- `log-record` and `log-full` print ticks and links, which vary by run. The `log` judge masks only the `t=`, `link=`, and `head=` fields, and only on header and summary lines. It then recomputes every masked value from the transcript itself: links from section 7, heads from the links, and ticks as non-decreasing. Everything else is byte-exact.
- Agent lines are sent only at an idle anchored prompt, and the shell acts on them only with an empty line buffer, so output order is fixed.
- `log-full` starts its one pending request about 1 s into a case whose total limit is 60 s. Its 60 s expiry therefore cannot print inside the case. A stuck case is 124, never a pass.
- No judge uses an exact tick value.

## 15. Smoke

`./scripts/smoke.sh` runs 68 QEMU cases, plus `harness-selftest`, and 28 build lines. The recorder build is `recorder-core`. The store build is `store-core`. The NVMe build is `nvme-core` (`docs/STORAGE.md`). `shell-basic`'s expected help text lists 24 lines. `expect_gate` takes an optional sixth argument: `exact` (the default), `log`, `store`, or `store:s1`.

The `log` judge (`judge_log` in the harness, bash wrapper `judge_shell_log status file expected want`):

1. `status == want`. The log has no `R3ATH3_PANIC` and no boot marker, and it decodes as ASCII.
2. Header lines match `^#([0-9]+) t=([0-9]+) (.+) link=([0-9a-f]{16})$`. The next line must start with two spaces; it is the body. Record `(n, tick, tag, body, link)`. In the comparison copy, rewrite the header as `#<n> t=T <tag> link=L`.
3. Summary lines match `^log entries=([0-9]+) of 128 lost=[0-9]+ head=([0-9a-f]{16})$` or `^replay ok entries=([0-9]+) lost=[0-9]+ head=([0-9a-f]{16})$`. Record `(k, head)`. Rewrite the hex as `H`.
4. The comparison copy must equal the expected file exactly. The expected file uses `T`, `L`, and `H` literally in those positions and nowhere else.
5. Every time entry `n` is printed, its tick, tag, body, and link are the same.
6. The printed `n` values are exactly `1`…`max`, with no gaps. `link_1 = link(BASIS, …)`, and each `link_n = link(link_{n-1}, …)`, all matching.
7. `tick_n >= tick_{n-1}`.
8. `k == 0` means `head` is the basis. Otherwise `k <= max` and `head` equals `link_k`.

| Case | Status | Harness, limit | What it proves |
| --- | --- | --- | --- |
| `recorder-core` | 245 | `expect_case`, 12 s | Stage line, the 11 step lines of section 12 in order, `R3ATH3_RECORDER_CORE ok steps=11`. Exact log. |
| `log-record` | 209 | `expect_gate … log`, 20 s | The transcript below. Nine of the 11 gate event kinds, human ok and cmd entries, an agent attempt to use a `log` verb, `replay`, `replay N`, and `log`. |
| `log-full` | 209 | `expect_gate … log`, 60 s | The generator below. Agent refused at the reserve (`reason=log-full`); human never locked out; full notice once; `approve` refused when full and the pending `nap` never runs; `deny` allowed; `lost` counts; a 128-entry `replay`. |
| `log-usage` | 211 | `expect_shell … exact`, 12 s | Usage and missing-entry errors. |
| `shell-selftest` | 209 | `expect_shell … exact`, 12 s | The all-pass card, and `selftest` leaves the live gate and task table alone. |

`log-usage` steps are `log x`, `replay 1 2`, `replay x`, `replay 0`, `replay 9`, `selftest x`, `exit`. At `replay 9` the record holds 4 entries. Output after the prefix and the first prompt:

```
log x
error: usage: log
r3> replay 1 2
error: usage: replay [N]
r3> replay x
error: no log entry x
r3> replay 0
error: no log entry 0
r3> replay 9
error: no log entry 9
r3> selftest x
error: usage: selftest
r3> exit
R3ATH3_SHELL_EXIT last=cmd
```

`shell-selftest` steps are `grant ai1 read`, `selftest`, `badges`, `ps`, `exit`. Output after the prefix and the first prompt:

```
grant ai1 read
granted ai1 read
r3> selftest
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
r3> badges
badge ai1 may=read
no pending request
r3> ps
task id=0 name=shell state=running
task id=1 name=idle state=ready
r3> exit
R3ATH3_SHELL_EXIT last=ok
```

`log-record` steps, in order: `log`, `grant ai1 spawn`, `grant ai1 say`, `R3GATE ASK ai1 1 say hello`, `R3GATE ASK ai1 2 log`, `R3GATE ASK zed 3 read`, `R3GATE ASK ai1 4 spawn nap`, `approve 9`, `approve 1`, `wait 2`, `R3GATE ASK ai1 5 spawn spin`, `deny 2`, `R3GATE ASK ai1 6 spawn nap`, `revoke ai1 spawn`, `replay`, `replay 11`, `log`, `exit`.

The expected file uses `T`, `L`, and `H` for ticks, links, and heads. After the usual three-line prefix and the prompt, the transcript is: an empty `log` (`entries=0 of 128 lost=0 head=H`); `granted ai1 spawn`; `granted ai1 say`; `ai1 says: hello` and `R3GATE reply agent=ai1 seq=1 verb=say result=done`; a bad line for verb `log` (`agent ai1` / `- seq=2 bad reason=verb`); a refused `zed` read (`reason=no-badge`); a pending `spawn nap` for rq 1; `error: no pending request 9`; `gate: request 1 approved: started task 2 (nap)` and the done reply with `id=2`; `exited id=2`; a pending `spawn spin` for rq 2; `gate: request 2 denied` and `reason=human`; a pending `spawn nap` for rq 3; `revoked ai1 spawn`; `gate: request 3 cancelled: ai1 may no longer spawn` and `reason=revoked`; `replay` of entries 1–20 then `replay ok entries=20 lost=0 head=H`; `replay 11` printing entry 11 (`gate` / `ai1 spawn-nap seq=4 rq=1 done id=2`) then `replay step 11 of 21 ok`; `log entries=22 of 128 lost=0 head=H` and entries 18–22; `R3ATH3_SHELL_EXIT last=ok`.

Stored bodies for those 22 entries, in order:

1. `human last=ok` / `log`
2. `gate` / `grant ai1 spawn`
3. `human last=ok` / `grant ai1 spawn`
4. `gate` / `grant ai1 say`
5. `human last=ok` / `grant ai1 say`
6. `agent ai1` / `say seq=1 done text=hello`
7. `agent ai1` / `- seq=2 bad reason=verb`
8. `agent zed` / `read seq=3 denied reason=no-badge`
9. `agent ai1` / `spawn-nap seq=4 rq=1 pending`
10. `human last=cmd` / `approve 9`
11. `gate` / `ai1 spawn-nap seq=4 rq=1 done id=2`
12. `human last=ok` / `approve 1`
13. `human last=ok` / `wait 2`
14. `agent ai1` / `spawn-spin seq=5 rq=2 pending`
15. `gate` / `ai1 spawn-spin seq=5 rq=2 denied reason=human`
16. `human last=ok` / `deny 2`
17. `agent ai1` / `spawn-nap seq=6 rq=3 pending`
18. `gate` / `revoke ai1 spawn`
19. `gate` / `ai1 spawn-nap seq=6 rq=3 denied reason=revoked`
20. `human last=ok` / `revoke ai1 spawn`
21. `human last=ok` / `replay`
22. `human last=ok` / `replay 11`

The final `log` and `exit` are not stored: `exit` does not return, and `log`'s own entry is appended after `log` prints.

`log-full` steps, in order:

1. `grant ai1 spawn`. Writes #1 (`gate` / `grant ai1 spawn`) and #2 (`human last=ok` / `grant ai1 spawn`).
2. `R3GATE ASK ai1 1 spawn nap`. The three pending lines for rq 1. Writes #3 (`agent ai1` / `spawn-nap seq=1 rq=1 pending`).
3. For `k` = 2…118 (117 lines): `R3GATE ASK zed <k> read`. Each is refused with `reason=no-badge`. Writes #4…#120 (`agent zed` / `read seq=<k> denied reason=no-badge`, with `k = n - 2`). Eight slots remain, which is the reserve.
4. `R3GATE ASK zed 119 read`. A blank line, then `gate: ignored an agent line: flight recorder full`, then `R3GATE reply agent=zed seq=119 verb=read result=denied reason=log-full`. `lost` becomes 1. No entry is stored.
5. Seven times `echo a`. Each prints `a`. Writes #121…#127 (`human last=ok` / `echo a`).
6. `echo b`. Prints `b`, then the full notice. Writes #128 (`human last=ok` / `echo b`).
7. `approve 1`. Prints `error: flight recorder full: approve refused`. The nap never runs. `lost` becomes 2.
8. `ps`. Prints ids 0 and 1 only. `lost` becomes 3.
9. `deny 1`. Prints `gate: request 1 denied` and `R3GATE reply agent=ai1 seq=1 verb=spawn result=denied rq=1 reason=human`. Both the gate entry and the human entry fail to store. `lost` becomes 5.
10. `replay`. Prints #1…#128, then `replay ok entries=128 lost=5 head=H`. The replay's own human entry is lost. `lost` becomes 6.
11. `log`. Prints `log entries=128 of 128 lost=6 head=H`, then #124…#128 (four `echo a` lines and `echo b`). `lost` becomes 7. That last human entry is not in the printed summary.
12. `exit`. Prints `R3ATH3_SHELL_EXIT last=ok`.

The harness self-test rejects a `recorder-core` log judged at 243, a log with one line removed, two lines swapped, or one link hex digit changed. It accepts `fill_log` samples of `log-record` and `log-full` (tick `10·n`, links and heads recomputed) against their own expected files, and rejects a changed link digit, a changed body byte, swapped ticks on #3 and #4, a head replaced by another entry's link, a second printing of #11 with a different tick, a gap at #5, the correct `log-record` sample judged at 211, `log-usage` at 209, and `shell-selftest` at 211. Shared-status swaps that must fail: filled `log-record` against `log-full`, filled `log-full` against `log-record`, `gate-approve` judged by the `log` judge against `log-record`, `shell-selftest` against `shell-basic`, and `log-usage` against `shell-usage` and against `gate-usage`. Every earlier self-test stays as it is.

Shared statuses: 209 covers 13 cases (`shell-basic`, `shell-backspace`, `shell-run`, `shell-mem`, `shell-screen`, `shell-ps`, `shell-wait`, `gate-approve`, `gate-noperm`, `gate-revoke`, `log-record`, `log-full`, `shell-selftest`). 211 covers 7 (`shell-unknown`, `shell-sched-error`, `shell-usage`, `gate-sneak`, `gate-usage`, `shell-sched-args`, `log-usage`). 213 covers 2, 217 covers 2, 163 covers 11, and 35 covers 2. 245 is one case. Shell-build cases are 25.

## 16. Try it by hand

Build the shell image (`./scripts/build-apps.sh` from the repo root, then `cargo build --release --features "shell glue-baked"` in `kernel/`). The kernel build fails unless that script has written `build/apps/glue_duty.wasm`. Boot it in QEMU the way `docs/SHELL.md` describes. Use the gate as in `docs/GATE.md`: `grant`, then an agent line on COM1, then `approve` or `deny`. Type `log` to see the newest entries and the totals, `replay` to check the chain and print every step, and `selftest` to print the report card. The record disappears on reboot.
