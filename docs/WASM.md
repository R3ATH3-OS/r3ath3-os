# WebAssembly runtime

Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".

This document is the spec for the runtime in this tree (milestone 11, part A and part B). If the code and this document disagree, the change is not done.

There are no crates.io dependencies. The toolchain is rustc/cargo 1.83.0. The kernel target is `x86_64-unknown-none`. Glue Duty is also built for `wasm32-unknown-unknown`. Both targets are in `rust-toolchain.toml`, and the Cloud Agents environment install line in `.cursor/environment.json` installs both. The work runs in QEMU only. Nothing here touches real hardware. There is no framebuffer and no sound (milestone 12). A browser host is not shipped.

## 1. What this tree runs

One integer WebAssembly interpreter in the kernel. One instance at a time, in a static arena. Module bytes stay in the kernel image; the interpreter borrows them and does not copy them into the arena. Nothing in the runtime or the app host allocates on the heap.

The shell command `app` can list three baked apps, `glue-duty`, `hello`, and `spin`, or run one of them. `glue-duty` is baked only into the `shell` and `wasm-glue` images. An app has no imports. It cannot call the kernel, read the clock, touch the store, COM1, the gate, or another task. The host reads two regions of the app's linear memory (the text grid and the summary) and writes the grid to the VGA text buffer.

The human gate is unchanged except for one sentence in `docs/GATE.md`. No verb starts, feeds, or reads an app. The gate is kernel-enforced permissions, default deny. The AI stays on the host and reaches the kernel over COM1. Agent names are self-asserted. The flight recorder is the record of gate outcomes and human commands. `app` is not a recorder event of its own: the human line is recorded by the existing `record_human` call after the app has ended. Keys pressed inside an app are not recorded.

FNV-1a, where this kernel uses it, is a checksum, not a cryptographic hash and not a security check. `wasm-glue` uses it to compare two grids. This runtime does not hash modules.

## 2. Files

| File | Role |
| --- | --- |
| `kernel/src/wasm/mod.rs` | `LoadError`, `Trap`, `Instance`, `with_instance` |
| `kernel/src/wasm/decode.rs` | One-pass structural validator and the side table |
| `kernel/src/wasm/exec.rs` | The interpreter loop. Calls use arena frames, not Rust recursion |
| `kernel/src/wasm/arena.rs` | The static arena. The only new privileged code |
| `kernel/src/apps.rs` | `hello`, `spin`, `glue-duty` on the shell and glue builds, the ABI check, and the shell host loop |
| `kernel/src/wasmtest.rs` | The `wasm-core` proof |
| `kernel/src/gluetest.rs` | The `wasm-glue` proof. No privileged code |
| `apps/glue-duty/` | The game. `logic.rs` and `text.rs` forbid `unsafe`. `abi.rs` is the wasm exports |
| `apps/glue-duty/src/bin/replay.rs` | The native host. Same source, same key script |
| `scripts/build-apps.sh` | Builds `glue_duty.wasm` and `glue-duty-replay` |

`kernel/Cargo.toml`: `wasm` is internal and is not a test mode. `wasm-core = ["wasm"]` and `wasm-glue = ["wasm"]` are test modes. Both are in the at-most-one assert and in both `boot_ok` exclusion lists. `shell` includes `wasm` and does not include `wasm-glue`.

`vga::present` paints an 80×25 grid. `qemu_exit::wasm_core_ok` writes `0x7D` (status 251). `qemu_exit::wasm_glue_ok` writes `0x7E` (status 253). Neither adds a privileged line.

## 3. The subset

Integers only: `i32` and `i64`, plus the sign-extension opcodes `0xC0`–`0xC4`. No floats, no imports, no WASI, no SIMD, no bulk memory, no reference types beyond the `funcref` table element type `0x70`, no multi-value, no threads.

Block types are `0x40` (empty), `0x7F` (one `i32` result), and `0x7E` (one `i64` result). A type-index block type, including `0x00`, is `opcode`. Float block types `0x7C` and `0x7D` are `float`. Every block, loop, and if in a module this runtime accepts has no parameters and zero or one result, so a branch unwinds to a known stack height.

`call_indirect` immediates are a type index (LEB) and then a table index (LEB). Only table 0 is accepted. `memory.size` and `memory.grow` each take one reserved byte, which must be `0x00`.

The validator is structural. It checks section shapes, indices, block nesting, branch depths, `br_table` lengths, local and global indices and mutability, and memory and table limits. It does not prove operand-stack types. Operand underflow traps; other type mismatches compute on the low bits. This is a sandbox property, not a claim of full spec conformance.

`else` is not its own label. During validation the control stack is stored in `arena.values` (that array is not the operand stack yet): the low 32 bits are the side index of the `block`, `loop`, or `if`, and the high 32 bits are the side index of its `else`, or `u32::MAX` when there is no else. An `if`'s side-table target is the `else` opcode when one exists, and the `end` opcode otherwise. An `else`'s target is the `end`. At run time, if the byte at an `if`'s target is `0x05`, that is the else.

The side table is appended in scan order, so it is sorted by code offset. Lookup is a binary search. `br_table` gets a side entry whose target is the program counter after the instruction. That entry is not a control-stack frame. The runtime reads the `br_table` immediates itself.

A branch whose depth equals the number of open blocks targets the function's own end label. Validation accepts it. At run time it is a return: the function result is popped, if the function has one, and the frame unwinds. A depth past that is `opcode` at load time. Spike W3's `glue_duty.wasm` emits none of these branches. The runtime accepts them so a later module that does emit one still returns.

Section ids are not accepted in strictly increasing order. The rank is: type 1, import 2, function 3, table 4, memory 5, global 6, export 7, start 8 (always `section`), element 9, data count 12 at rank 10, code 10 at rank 11, data 11 at rank 12. A custom section (id 0) is skipped wherever it appears. A repeated or out-of-order ranked section is `section`. A start section is `section`. An import section with a count above 0 is `import`. An empty import section is accepted.

## 4. Limits and errors

Load-time `LoadError` words: `magic`, `version`, `truncated`, `section`, `opcode`, `float`, `import`, `limits`, `export`, `too-big`, `busy`. `busy` is not a module fault. It is the word the shell prints when `with_instance` returns `None` because the arena is already taken. `limits` stays the word for a module that exceeds a limit. `wasm-core` step 8 still lists `limits` and does not list `busy`.

| Item | Limit |
| --- | --- |
| Module bytes | 65,536 (`too-big`) |
| Types | 128 |
| Functions | 512 |
| Locals per function, params included | 256 |
| Globals | 64 |
| Table entries | 512, and a declared maximum is required |
| Exports | 64, and every name is at most 32 bytes (`export` when longer). An empty name is allowed. Duplicate names are `export` |
| Side-table entries | 16,384 (`limits`) |
| Linear memory | declared maximum at most 4 pages (256 KiB). Initial size is at most that maximum. A maximum is required. More than one memory, or more than one table, is `limits` |

Run-time `Trap` words: `unreachable`, `memory`, `div-zero`, `overflow`, `indirect`, `table`, `stack`, `fuel`, `dead`.

| Item | Limit |
| --- | --- |
| Operand stack | 8,192 values |
| Call frames | 256 |
| Open labels | 2,048 |
| Fuel | 1,000,000 instructions per `call`. `call_with_fuel` can pass a smaller budget |

`pop` and `peek` stop at the operand floor. That floor is the end of the current frame's locals (`locals_base + local_count`). When a label is open, the floor is the greater of that end and the top label's `stack_base`. A function or a block that pops more than it pushed traps `stack`. It does not read its locals or the caller's slots. The return pop and the block-end pop run while that frame or label is still the top, so the same floor applies. `enter` uses that floor while the caller's frame is still on top: a `call` or `call_indirect` traps `stack` when `sp` is below the floor plus the parameter count, and does not take a missing argument from a local. A returned `i32` is the low 32 bits of the slot, sign-extended back into the `i64`. A function typed `-> i32` cannot hand the host a raw `i64` such as `i64::MAX`.

After any trap the instance is dead: every later `call` returns `dead`. A missing export, or the wrong number of arguments, is also `dead` or `stack` from that call, and the instance is then dead. Integer division or remainder by zero is `div-zero`. `i32.div_s` and `i64.div_s` of the minimum value divided by −1 are `overflow`. The matching remainder is 0. Shifts and rotates use wrapping shifts, so the count is masked the way Rust masks it.

A memory address is `(addr as u32 as u64) + offset as u64`. If the end of the access is past the current size, the trap is `memory`. `memory.grow` returns the old page count, or −1 when the result would pass the declared maximum or 4 pages, and it zeros the new range. `i32` values are stored sign-extended in an `i64` slot. `i64.clz`, `i64.ctz`, and `i64.popcnt` push an `i32`.

An unknown opcode at run time is `unreachable`, not a panic. A bad module is a `LoadError` or a `Trap`. The interpreter does not panic on module bytes.

## 5. The arena

`Arena` is `repr(C)` and lives in `.bss`, zeroed at boot, never on the heap, never freed. `size_of::<wasm::arena::Arena>()` is **513,376** bytes. A const assert rejects a size above 524,288. The fields are 256 KiB of linear memory, 8,192 `i64` values, 256 frames, 2,048 labels, 16,384 side entries, 64 globals, a 512-entry table, 512 function infos, 128 types, and 64 exports. The data-segment count for the optional data-count section is stashed in the two pad bytes of the header (at most 65,535 data segments).

`with_arena` is the only borrower. `TAKEN` is an `AtomicBool` set with `Acquire` before the borrow and cleared with `Release` before the function returns. The two new privileged lines in the kernel are both in `kernel/src/wasm/arena.rs`:

```rust
unsafe impl Sync for ArenaCell {}
let result = f(unsafe { &mut *ARENA.0.get() });
```

`rg -n 'unsafe|asm!' kernel/src` is 122 lines. Those two are the only lines this milestone adds. Comments in that file do not contain the substring `unsafe`.

On the shell build, `mem` reported `frames_total=15943` on main (`3ac0fe5`) and `frames_total=15773` on the part A head. The drop is 170 frames, inside the 128–176 bound. Part B may drop at most 24 more, so the shell build's `frames_total` must sit in 15749..=15773. The measured part B figure is `frames_total=15765`, a drop of 8 from 15773, inside that bound. Builds without `wasm` do not include the arena. The `shell-mem` case checks that bound. `judge_shell_mem` itself does not: its self-test still uses a synthetic `frames_total` of 16000.

The shell's slot 0 stack is 16 KiB and has no canary. This tree adds no assembly to measure the high-water mark, so that mark is not verified. The interpreter does not recurse in Rust.

A fresh load zeros the pages the previous instance had committed and the table slots it had used, then `parse_memory` zeros the new initial pages before data segments are copied. A module that wrote a word reads 0 at that address after the next load. The whole 256 KiB is not wiped on every load.

## 6. API

```rust
pub enum LoadError { Magic, Version, Truncated, Section, Opcode, Float, Import, Limits, Export, TooBig, Busy }
pub enum Trap { Unreachable, Memory, DivZero, Overflow, Indirect, Table, Stack, Fuel, Dead }
pub struct Instance<'m, 'a> { /* borrows the module bytes and the arena */ }
impl Instance {
    pub fn call(&mut self, export: &str, args: &[i64]) -> Result<Option<i64>, Trap>;
    pub fn call_with_fuel(&mut self, export: &str, args: &[i64], fuel: u32) -> Result<Option<i64>, Trap>;
    pub fn call_metered(&mut self, export: &str, args: &[i64], fuel: u32) -> (Result<Option<i64>, Trap>, u32);
    pub fn memory(&self) -> &[u8];
    pub fn has_export(&self, name: &str) -> bool;
    pub fn export_is_memory(&self, name: &str) -> bool;
    pub fn func_matches(&self, name: &str, params: &[ValType], result: Option<ValType>) -> bool;
}
pub fn with_instance<R>(
    module: &[u8],
    f: impl FnOnce(Result<Instance<'_, '_>, LoadError>) -> R,
) -> Option<R>;
```

`with_instance` returns `None` when the arena is already taken. That does not happen in this milestone: one shell, one app. A load failure is `Some(Err(_))`. The module slice lives as long as the call. Baked apps are `'static`. The flip test in `wasm-core` passes a stack copy. `call_metered` returns that result and the fuel used (`budget` minus what remains). `wasm-glue` calls `r3_step` through it and fails the proof with `detail=fuel-budget` when one step uses more than 250,000. Nothing about fuel is printed. `None` from `call` means the export returns no value. `Some` is an `i32` or `i64` result. An `i32` result is the sign-extended low 32 bits, as the operand-floor paragraph describes.

Fuel is 1 per opcode, counted before the opcode runs. `br_table` then charges one more unit for each of its `n` label immediates, before those immediates are read. If fewer than `n` units remain, fuel goes to 0 and the trap is `fuel`. The default label is one extra LEB and is not charged beyond the opcode, the same as `br`. A huge table cannot be decoded for a single fuel unit. Running out is `fuel`, which stops the instance. There is no resume. A module that wants to keep going must return and be called again. The shell build preempts on the timer tick. With only the shell and the idle task, the scheduler keeps running the shell, so a long call is not switched away from, and the tick counter still advances. At the assumed 2 million instructions per second the fuel budget is about half a second. Spike W1, on this QEMU TCG host, measured well above that floor: a counted loop of 3,600,004 opcodes (`result` equal to the iteration count) printed `ips=18000020` at `ticks=20` and `hz=100` on the saved run, and `ips=16363654` at `ticks=22` on the run before it. Both are above 2,000,000 per second. The logs are in the pull request. Spike W2's `app spin` printed `stopped: trap fuel` in under a fifth of a second after the command was entered, inside the 2 second bound.

## 7. The `r3app` ABI v1

Exports only:

| Export | Signature | Meaning |
| --- | --- | --- |
| `memory` | memory | Linear memory, at most 4 pages |
| `r3_abi` | `() -> i32` | Must return 1 |
| `r3_init` | `(seed: i32)` | The host passes `ticks() as i32 \| 1` |
| `r3_key` | `(key: i32)` | One key: a byte `0x20`–`0x7E`, or 8 for Backspace, or 10 for Enter. Never `q`. Capital `Q` is passed through |
| `r3_step` | `() -> i32` | One fixed step. The state is the returned `i32` |
| `r3_text` | `() -> i32` | Offset of a 2,000-byte grid, 80 by 25, row-major |
| `r3_summary` | `() -> i32` | Offset of a length byte (0–60) and that many following bytes |

Grid bytes: `0x00` is a space, `0x20`–`0x7E` are themselves, anything else is `?`. The host writes attribute `0x0A` (green on black). The same glyph rule is used for a non-empty summary.

`apps::check_abi` requires all seven exports with those kinds and signatures, then requires `r3_abi()` to return 1.

### 7.1 Host loop

`apps::run_app` on the shell build:

1. Print `app <name>: started; q quits` on the console and on serial.
2. Load. On `LoadError`, print `app <name>: not loaded: <word>` and return. A taken arena is `not loaded: busy`. `last` becomes cmd.
3. ABI check. On failure, print `app <name>: stopped: abi` and return. `last` becomes cmd.
4. `r3_init`. A trap stops the app.
5. Paint the grid from `r3_text` (the data segment is already applied). An out-of-range offset is `memory`.
6. Call `r3_step` once. A trap stops the app here, before the ready line and before any state line. `spin` takes this path: the first step is the infinite loop, fuel runs out, and the end line is `app spin: stopped: trap fuel`.
7. Remember that state. It is the baseline and is not printed.
8. Print `R3ATH3_APP_READY name=<name>` on serial only. The screen is already painted, so the harness can `xp` as soon as it sees this line.
9. Loop, one step per 10 ticks. While waiting, drain `pop_key` and `yield_now`. `q` ends the app. Any other `Char`, Backspace, or Enter is passed to `r3_key` in order. Then `r3_step`. If the state differs from the baseline or from the previous printed state, print `R3ATH3_APP_STATE name=<name> state=<s>` on serial only, then paint the grid.
10. Clear the VGA text buffer and print one line on both channels. `hello`'s summary length is 0, so quit prints `app hello: quit`. A length from 1 to 60 prints `app <name>: quit; <text>`. A trap prints `app <name>: stopped: trap <word>`.

A quit and a trap both set `last` to ok. The shell then prints `r3> ` the way it does after any other command. It does not reprint the banner.

While the app runs, the shell is inside `dispatch`. Agent bytes stay in the COM1 buffer. The gate's pending-request expiry is not polled until the shell loop resumes, the same as during `wait`.

## 8. Shell surface

`app` with no argument lists the baked apps, name padded to 11 characters:

```
app glue-duty  Glue Duty, the belt-scrub game, in text mode
app hello      a static screen; the smallest app
app spin       loops forever; the fuel limit stops it
```

`app a b` prints `error: usage: app [NAME]`. `app nope` and `app Spin` print `error: no app <NAME>`. Those set `last` to cmd.

Help stays 24 lines. The `app [NAME]` line is after `forget`, and `exit` and `halt` share one line: `exit, halt  leave with the last status, or stop the machine`. Descriptions still start at column 12. `selftest` is still 8 checks.

`hello` places `hello from wasm` at row 12 column 30 (offset 990) and `press q to quit` at row 14 column 30 (offset 1150). The rest of the grid is spaces.

## 9. `wasm-core`

Status 251, port byte `0x7D`, `qemu_exit::wasm_core_ok`. A failed check prints `R3ATH3_WASM_CORE fail step=<n> <detail>` and then `qemu_exit::fail` (status 35). It does not panic. The serial log, including the stage line `kernel_main` always prints, is:

```
R3ATH3_STAGE rust_main
R3ATH3_WASM_CORE start
R3ATH3_WASM_CORE step=1 add=42
R3ATH3_WASM_CORE step=2 fac64=2432902008176640000 ext8=-1
R3ATH3_WASM_CORE step=3 fib=6765 pick=10,20,99 ind=7,8
R3ATH3_WASM_CORE step=4 mem=305419896 grow=1,-1 size=2
R3ATH3_WASM_CORE step=5 traps=unreachable,memory,div-zero,overflow,indirect,stack,fuel
R3ATH3_WASM_CORE step=6 after-trap=dead fresh=42
R3ATH3_WASM_CORE step=7 fresh-memory=0
R3ATH3_WASM_CORE step=8 refused=magic,version,truncated,section,opcode,float,import,limits
R3ATH3_WASM_CORE step=9 prefixes=ok flips=ok
R3ATH3_WASM_CORE step=10 hello=abi-ok spin-init=fuel
R3ATH3_WASM_CORE step=11 heap=same frames=same
R3ATH3_WASM_CORE ok steps=11
```

1. `add(40, 2)` in `i32`.
2. `fac64(20)` is 20! = 2432902008176640000. `ext8(0xFF)` is `i32.extend8_s` and returns −1. `wide` is typed `-> i32` and its body is `i64.const` of `i64::MAX` (9223372036854775807). The host result is −1, not that raw `i64`.
3. Recursive `fib(20)` is 6765, through arena frames. `pick` is `br_table` with empty blocks: index 0 returns 10, index 1 returns 20, an index past the table returns 99. `ind` is `call_indirect` through table slots 0 and 1 and returns 7 and 8.
4. Store `0x12345678` and load it back (305419896). `memory.grow(1)` from 1 page with maximum 2 returns 1. A second grow returns −1. `memory.size` is 2.
5. Seven separate instances, one trap each, in that word order. The same step then loads a module whose caller stores 222 at offset 0 and calls a function with argument 333. That function stores the parameter at offset 4 and a local 111 at offset 8, then executes `i32.add` on an empty operand stack, keeps the sum, and `i32.store`s it at offset 12 (`i32.const 12` is the address, and the sum is the value). The add traps `stack`. The three stored words stay, and offset 12 stays 0. The same step also loads a caller that sets a local and, with an empty operand stack, calls a one-parameter function. That call traps `stack`.
6. After `unreachable`, the next call on that instance is `dead`. A new load of `add` returns 42.
7. A module writes 99, and the next load reads 0 at the same address.
8. Eight malformed modules, refused with those words: bad magic, version 2, a section id with no size, a start section, opcode `0x06`, `f32.const`, one import, memory maximum 5.
9. Every proper prefix of the `add` module is loaded or refused without a panic, and none of them returns 42 from `add`. A prefix that is itself a well-formed module does load. The 8-byte magic and version is an empty module; calling `add` on it does not return 42. Each byte of the module is flipped to each of `0x00`, `0x01`, `0x0B`, `0x20`, `0x41`, `0x7F`, `0x80`, and `0xFF`. The flipped bytes either load and `call_with_fuel("add", [40, 2], 10_000)` returns, or the load or the call is refused. The proof requires that this finishes. It does not require the numeric result to stay 42, because a flip can turn `i32.add` into another accepted opcode.
10. Baked `hello` passes the ABI check. Baked `spin` traps `fuel` on its first `r3_step`.
11. `heap::used()` and the free frame count are the same before step 1 and after step 10.

## 10. Smoke

`./scripts/smoke.sh` runs 75 cases and 32 builds. Part A added the build `wasm-core` and the cases `wasm-core` (251), `app-hello` (209), `app-spin` (209), `app-usage` (211), and `app-gate` (209). Part B adds the builds `glue-duty-wasm`, `glue-duty-replay`, and `wasm-glue`, and the cases `wasm-glue` (253) and `app-glue` (209). `build_app` runs `scripts/build-apps.sh` before `wasm-glue` and `shell`, because those images bake `build/apps/glue_duty.wasm`. `wasm-core` does not bake it. `help_text` is unchanged. The `app` list in `app-spin` grows by the `glue-duty` line.

The harness self-test rejects a `wasm-core` log with `add=41`, a log missing the `ok` line, and the good log judged at status 249. It rejects an `app-hello` VGA dump with one cell changed, an `app-spin` log that says `trap stack`, and an `app-gate` log that inserts `R3ATH3_APP_READY` after the agent's `say`. It rejects a `wasm-glue` replay whose `frame=` differs, a golden whose `frame=` differs, a truncated kernel log, and an `app-glue` title dump with one cell changed. After `nvme-core` and `wasm-core` have both run, each log fails the other proof's judge at the wrong status and again at the right status, so a matching body cannot hide behind the status check. `wasm-glue`'s log judged as `wasm-core` fails. Those checks do not print their own `[PASS]` lines.

`app-gate` checks three refusals and one human run. `grant ai1 app` prints `error: unknown permission: app`. `R3GATE ASK ai1 1 app spin` is a bad line with `reason=verb`. `say` of the text `app spin` is printed as `ai1 says: app spin` and does not print `R3ATH3_APP_READY`. The human `app spin` then traps `fuel`. Badges still show `say` only.

## 11. Glue Duty

The game is one Rust source under `apps/glue-duty/`. `logic.rs` and `text.rs` are `#![forbid(unsafe_code)]`. `abi.rs` has two lines containing `unsafe`: `unsafe impl Sync` on its one `UnsafeCell`, and the block that borrows it. The panic handler calls `core::arch::wasm32::unreachable()`, which is a safe function at rustc 1.83.0, so the count stays 2. The crate is `no_std` only on `wasm32` (`#![cfg_attr(target_arch = "wasm32", no_std)]`). The package also builds a host cdylib for the replay binary, and a host `no_std` cdylib would need a second panic handler. The wasm artifact is `no_std`.

`scripts/build-apps.sh` changes to `apps/glue-duty` before `cargo build`. Cargo reads `.cargo/config.toml` from that directory and passes `-zstack-size=32768`, `--initial-memory=131072`, and `--max-memory=262144`. The module's memory is 2 pages initial and 4 pages maximum. There is no import section. Block types in this module are `0x40` only. Spike W3's report is in the pull request. The module is 15,797 bytes.

The rules are the integer port in `logic.rs`: three lanes, a 72-column belt, a scraper fixed at belt column 8, shifts of 750 steps, at most 24 blobs, xorshift32, and no floats. `text.rs` paints the 80×25 grid. Every store goes through `store_byte`, which is `#[inline(never)]`, so the module does not emit `memory.fill` or `memory.copy`. `q` is never delivered. Pixels and sound are milestone 12.

The title grid, after `r3_init(1)`, is the `app-glue` VGA golden (`apps/glue-duty/golden/app-glue.vga`). Attribute `0x0A`. The text is:

```
GLUE DUTY  shift 1  time 1:15  score 0  cleaned 0  solvent 2  combo x1
gum-up [....................]   0%
    scrub before the glue ships                                 DISCHARGE ->

    ================================================================!!!!!!!!
 1  ----------------------------------------------------------------::::::::
    ========v=======================================================!!!!!!!!
 2  ----------------------------------------------------------------::::::::
    ================================================================!!!!!!!!
 3  ----------------------------------------------------------------::::::::
    ================================================================!!!!!!!!

Three lanes. Scrub glue, before it ships!
Scrub with a and d. Alternate strokes cut deeper. Space is solvent.
Keep the discharge clear. A blob that slips through gums the whole belt.
Survive the shift. The next one is faster and cleaner.
Press space to start work.

  log: Nothing yet. The belts are suspiciously clean.

w s: lane   a d: scrub (alternate)   space: solvent   p: pause   q: quit
```

Blank rows are empty. The `v` is the scraper.

## 12. `wasm-glue`

Status 253, port byte `0x7E`, `qemu_exit::wasm_glue_ok`. A failed check prints `R3ATH3_WASM_GLUE fail step=<n> <detail>` and then `qemu_exit::fail` (status 35). The kernel log, the native `replay` stdout, and `apps/glue-duty/golden/wasm-glue.txt` are byte-identical. The kernel also prints `R3ATH3_STAGE rust_main` before the proof, and `replay` prints that same line so the three copies match.

```
R3ATH3_STAGE rust_main
R3ATH3_WASM_GLUE start seed=1
R3ATH3_WASM_GLUE step=<n> state=<s> frame=<fnv64 of the 2000-byte grid, 16 hex digits> <summary>
R3ATH3_WASM_GLUE ok steps=2000
```

A checkpoint is printed every 100 steps and on every state change. `frame` is FNV-1a 64 over the raw grid (basis `0xcbf29ce484222325`, prime `0x100000001b3`), a checksum for comparing two outputs, not a security check. The summary is `shift k score n cleaned c gum-up p%` with `p = jam / 100`.

Seed is 1. Keys are applied before step `n` (`n` from 1), in `replay.rs` and again in `gluetest.rs`:

- `n = 1` and `n = 1001`: Space.
- For `2 ≤ n ≤ 740` and `1002 ≤ n ≤ 1400`, in this order: `w` when `n % 60 == 0` and `(n/60) % 2 == 0`, else `s` when `n % 60 == 0`; Space when `n % 150 == 75`; `p` when `n` is 300 or 310; then `a` when `n` is even, else `d`.
- No keys otherwise.

Each `r3_step` is given the full 1,000,000 fuel budget and must use at most 250,000. The proof does not print the fuel count.

`app-glue` launches `glue-duty`, dumps the title screen, sends Space (state 1), `p` (state 2), `p` (state 1), and `q`. The end line is `app glue-duty: quit; shift 1 score 0 cleaned 0 gum-up 0%`. Quit is success, so the case exits 209. On this shell build `mem` reports `frames_total=15765`.
