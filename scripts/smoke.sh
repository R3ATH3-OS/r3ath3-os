#!/usr/bin/env bash
# Bounded QEMU boot smoke test. Exits 0 only when every case passes.
# QEMU isa-debug-exit maps port 0xF4 value V to process status (V << 1) | 1.
# Success writes 0x10 (status 33). A panic writes 0x11 (status 35).
# CPU faults: #UD 0x31→99, #PF 0x33→103, #DF 0x34→105.
# Any other vector writes 0x2F (status 95), outside timeout's 124–127.
# Task scripts write 0x50 (ok → 161) or 0x51 (error → 163).
# Timer proof 0x58 → 177. Keyboard echo 0x59 → 179. Keyboard script 0x5A → 181.
# Allocator proofs: heap 0x60 → 193, heap exhaustion 0x61 → 195, frames 0x62 →
# 197, frame exhaustion 0x63 → 199, memory init error 0x64 → 201. The heap
# overflow panic case reuses the panic status 35 on purpose.
# Shell: exit ok 0x68 → 209, command error 0x69 → 211, line overflow 0x6A → 213,
# script error 0x6B → 215, halt 0x6C → 217.
# Scheduler: coop 0x70 → 225, preempt 0x71 → 227, sleep 0x72 → 229,
# full 0x73 → 231, exit 0x74 → 233, reap 0x75 → 235.
# Gate: core 0x78 → 241, expire 0x79 → 243.
# Flight recorder: core 0x7A → 245. Store: core 0x7B → 247. NVMe: core 0x7C → 249.
# WebAssembly: core 0x7D → 251. Glue Duty: 0x7E → 253.
# With no NVMe controller the shell uses the RAM disk, and a reboot erases it.
# With QEMU's emulated NVMe disk, earlier boots stay in the image.
# Saved flight-recorder entries survive a reboot only inside that image.
# Unused odd statuses: 203–207, 219–223, 237–239, 255.
# A triple-fault or silent reset does not print the fault line and does not
# use those statuses (this QEMU build has exited 0 on triple fault).
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL_DIR="$ROOT/kernel"
OUT="$ROOT/build/smoke"
MARKER="R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation"
PASS_STATUS=33
FAIL_STATUS=35
TIMEOUT_STATUS=124
UD_STATUS=99
PF_STATUS=103
DF_STATUS=105
SCRIPT_OK_STATUS=161
SCRIPT_ERR_STATUS=163
TICKS_STATUS=177
KEYS_STATUS=179
KBD_SCRIPT_STATUS=181
HEAP_STATUS=193
HEAP_OOM_STATUS=195
FRAMES_STATUS=197
FRAMES_OOM_STATUS=199
MEM_ERROR_STATUS=201
SHELL_OK_STATUS=209
SHELL_CMD_STATUS=211
SHELL_OVERFLOW_STATUS=213
SHELL_SCRIPT_STATUS=215
SHELL_HALT_STATUS=217
SCHED_COOP_STATUS=225
SCHED_PREEMPT_STATUS=227
SCHED_SLEEP_STATUS=229
SCHED_FULL_STATUS=231
SCHED_EXIT_STATUS=233
SCHED_REAP_STATUS=235
GATE_CORE_STATUS=241
GATE_EXPIRE_STATUS=243
RECORDER_CORE_STATUS=245
STORE_CORE_STATUS=247
NVME_CORE_STATUS=249
WASM_CORE_STATUS=251
WASM_GLUE_STATUS=253
RAM_BYTES=67108864
HEAP_BYTES=1048576
HEAP_LINE='R3ATH3_HEAP ok size=1048576 used=([0-9]+) align_max=4096'
HEAP_OOM_LINE='R3ATH3_HEAP_OOM ok chunks=16 used=1048576'
HEAP_PANIC_LINE='R3ATH3_PANIC location=[^ ]+:[0-9]+ message=memory allocation of 2097152 bytes failed'
FRAMES_LINE='R3ATH3_FRAMES ok total=([0-9]+) free=([0-9]+) first=0x([0-9a-f]+) kernel_end=0x([0-9a-f]+)'
FRAMES_OOM_LINE='R3ATH3_FRAMES_OOM ok allocated=([0-9]+) max=0x([0-9a-f]+)'
MEM_ERROR_LINE='R3ATH3_MEM_ERROR reason=startinfo'
TICKS_LINE='R3ATH3_TICKS ok count=300 hz=100 rtc=[34]'
UD_LINE='R3ATH3_FAULT vector=6 name=ud error=none rip=0x[0-9a-f]+'
PF_LINE='R3ATH3_FAULT vector=14 name=pf error=0x0 rip=0x[0-9a-f]+ cr2=0x40000000'
DF_LINE='R3ATH3_FAULT vector=8 name=df error=0x0 rip=0x[0-9a-f]+'
FAILURES=0

mkdir -p "$OUT"

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "FAIL: required tool not found: $1"
    exit 1
  fi
}

require_tool cargo
require_tool rustc
require_tool qemu-system-x86_64
require_tool timeout
require_tool python3

log() { printf '%s\n' "$*"; }

judge_boot_ok() {
  local status="$1" file="$2"
  [[ "$status" -eq "$PASS_STATUS" ]] || return 1
  grep -F -x -q "$MARKER" "$file"
}

judge_panic() {
  local status="$1" file="$2"
  [[ "$status" -eq "$FAIL_STATUS" ]] || return 1
  grep -F -q "R3ATH3_PANIC" "$file" || return 1
  grep -F -q "injected boot fault" "$file" || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  return 0
}

judge_hang() {
  local status="$1" file="$2"
  [[ "$status" -eq "$TIMEOUT_STATUS" ]] || return 1
  grep -F -q "R3ATH3_HANG" "$file" || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  return 0
}

# Exact QEMU status plus one full fault line. Wrong status (including a
# triple-fault exit of 0) or a missing line (silent reset) is a failure.
judge_fault() {
  local status="$1" file="$2" expected="$3" pattern="$4"
  [[ "$status" -eq "$expected" ]] || return 1
  grep -E -x -q "$pattern" "$file" || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  return 0
}

judge_ud() { judge_fault "$1" "$2" "$UD_STATUS" "$UD_LINE"; }
judge_pf() { judge_fault "$1" "$2" "$PF_STATUS" "$PF_LINE"; }
judge_df() { judge_fault "$1" "$2" "$DF_STATUS" "$DF_LINE"; }

judge_ticks() {
  local status="$1" file="$2"
  [[ "$status" -eq "$TICKS_STATUS" ]] || return 1
  grep -E -x -q "$TICKS_LINE" "$file" || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  return 0
}

judge_heap() {
  local status="$1" file="$2"
  [[ "$status" -eq "$HEAP_STATUS" ]] || return 1
  local line
  line="$(grep -E -x -m1 "$HEAP_LINE" "$file")" || return 1
  [[ "$line" =~ ^${HEAP_LINE}$ ]] || return 1
  local used="${BASH_REMATCH[1]}"
  (( used > 0 && used <= HEAP_BYTES )) || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  return 0
}

judge_heap_oom() { judge_fault "$1" "$2" "$HEAP_OOM_STATUS" "$HEAP_OOM_LINE"; }

# The panic path is the proof, so this judge requires a panic line (status 35)
# and rejects the boot marker.
judge_heap_panic() {
  local status="$1" file="$2"
  [[ "$status" -eq "$FAIL_STATUS" ]] || return 1
  grep -E -x -q "$HEAP_PANIC_LINE" "$file" || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  return 0
}

judge_frames() {
  local status="$1" file="$2"
  [[ "$status" -eq "$FRAMES_STATUS" ]] || return 1
  local line
  line="$(grep -E -x -m1 "$FRAMES_LINE" "$file")" || return 1
  [[ "$line" =~ ^${FRAMES_LINE}$ ]] || return 1
  local total="${BASH_REMATCH[1]}" free="${BASH_REMATCH[2]}"
  local first=$((16#${BASH_REMATCH[3]})) kernel_end=$((16#${BASH_REMATCH[4]}))
  (( total > 0 && free == total )) || return 1
  (( first >= kernel_end )) || return 1
  (( total * 4096 <= RAM_BYTES )) || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  return 0
}

judge_frames_oom() {
  local status="$1" file="$2"
  [[ "$status" -eq "$FRAMES_OOM_STATUS" ]] || return 1
  local line
  line="$(grep -E -x -m1 "$FRAMES_OOM_LINE" "$file")" || return 1
  [[ "$line" =~ ^${FRAMES_OOM_LINE}$ ]] || return 1
  local allocated="${BASH_REMATCH[1]}" max=$((16#${BASH_REMATCH[2]}))
  (( allocated > 0 )) || return 1
  (( max + 4096 <= RAM_BYTES )) || return 1
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  return 0
}

judge_mem_badinfo() { judge_fault "$1" "$2" "$MEM_ERROR_STATUS" "$MEM_ERROR_LINE"; }

# Status 209, two identical mem lines, and one uptime line. The expected file
# uses the literal lines MEM and UPTIME where those three lines sit.
judge_shell_mem() {
  local status="$1" file="$2" expected="$3"
  [[ "$status" -eq "$SHELL_OK_STATUS" ]] || return 1
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  python3 - "$file" "$expected" <<'PY'
import re, sys
actual_path, expected_path = sys.argv[1:]
mem_re = re.compile(
    r"^mem heap_used=([0-9]+) heap_size=1048576 frames_total=([0-9]+) frames_free=([0-9]+)$"
)
up_re = re.compile(
    r"^uptime ticks=([0-9]+) hz=100 seconds=([0-9]+)\.([0-9]{2})$"
)
try:
    text = [line.decode("ascii") for line in open(actual_path, "rb").read().splitlines()]
    expected = [line.decode("ascii") for line in open(expected_path, "rb").read().splitlines()]
except UnicodeDecodeError:
    sys.exit(1)
mems = []
ups = 0
out = []
for line in text:
    mem = mem_re.match(line)
    up = up_re.match(line)
    if mem:
        used = int(mem.group(1))
        total = int(mem.group(2))
        free = int(mem.group(3))
        if used > 4096 or total <= 0 or free != total or total * 4096 > 67108864:
            sys.exit(1)
        mems.append(line)
        out.append("MEM")
    elif up:
        ticks = int(up.group(1))
        seconds = int(up.group(2))
        hundredths = int(up.group(3))
        if ticks <= 0 or seconds != ticks // 100 or hundredths != ticks % 100:
            sys.exit(1)
        ups += 1
        out.append("UPTIME")
    else:
        out.append(line)
if len(mems) != 2 or mems[0] != mems[1] or ups != 1:
    sys.exit(1)
if out != expected:
    sys.exit(1)
sys.exit(0)
PY
}

# Part B shell builds may drop at most 24 frames from the Part A figure
# 15773. This is not part of judge_shell_mem: that judge's self-test uses
# a synthetic frames_total of 16000 and must keep passing.
shell_frames_in_bound() {
  python3 - "$1" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="ascii", errors="replace").read()
nums = [int(n) for n in re.findall(r"frames_total=([0-9]+)", text)]
if len(nums) < 1 or len(set(nums)) != 1:
    sys.exit(1)
sys.exit(0 if 15749 <= nums[0] <= 15773 else 1)
PY
}

judge_shell_screen() {
  local status="$1" serial="$2" expected_serial="$3" vga="$4" expected_vga="$5"
  judge_exact "$status" "$serial" "$SHELL_OK_STATUS" "$expected_serial" || return 1
  [[ -f "$vga" && -f "$expected_vga" ]] || return 1
  cmp -s "$vga" "$expected_vga"
}

write_harness_py() {
  cat >"$OUT/harness_py.py" <<'PY'
import re

PROMPT = b"r3> "

def prompt_anchored(data):
    if not isinstance(data, (bytes, bytearray)) or not data.endswith(PROMPT):
        return False
    head = data[:-len(PROMPT)]
    return head.endswith(b"\n") or head.endswith(b"\x1b[H")

def idle_at_prompt(data, start, echo):
    if not isinstance(data, (bytes, bytearray)):
        return False
    if start < 0 or start > len(data):
        return False
    if (echo + b"\n") not in data[start:]:
        return False
    return prompt_anchored(data)

def keys_at_prompt(data, start):
    if not isinstance(data, (bytes, bytearray)):
        return False
    if start < 0 or start >= len(data):
        return False
    if b"\n" not in data[start:]:
        return False
    return prompt_anchored(data)

def _lines(text):
    if text.endswith(b"\n"):
        text = text[:-1]
    if text == b"":
        return []
    return text.split(b"\n")

def judge_sched_coop(text):
    lines = _lines(text)
    expect = [b"R3ATH3_STAGE rust_main"]
    for index, name in enumerate("ABC" * 3):
        step = index // 3 + 1
        expect.append(f"R3ATH3_SCHED_COOP task={name} step={step}".encode())
    if lines[:10] != expect or len(lines) != 11:
        return False
    match = re.fullmatch(rb"R3ATH3_SCHED_COOP ok order=ABCABCABC switches=([0-9]+)", lines[10])
    return bool(match) and int(match.group(1)) >= 9

def judge_sched_preempt(text):
    lines = _lines(text)
    if len(lines) != 7 or lines[0] != b"R3ATH3_STAGE rust_main":
        return False
    for step in range(1, 6):
        if lines[step] != f"R3ATH3_SCHED_PREEMPT worker step={step}".encode():
            return False
    match = re.fullmatch(
        rb"R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=([0-9]+) preemptions=([0-9]+)",
        lines[6],
    )
    if not match:
        return False
    return int(match.group(1)) > 0 and int(match.group(2)) >= 5

def judge_sched_sleep(text):
    lines = _lines(text)
    if len(lines) != 5 or lines[0] != b"R3ATH3_STAGE rust_main":
        return False
    order = (("S", 10), ("M", 20), ("L", 30))
    for index, (name, want) in enumerate(order):
        match = re.fullmatch(
            rb"R3ATH3_SCHED_SLEEP name=([A-Z]) want=([0-9]+) elapsed=([0-9]+)",
            lines[1 + index],
        )
        if not match or match.group(1).decode() != name or int(match.group(2)) != want:
            return False
        elapsed = int(match.group(3))
        if elapsed < want or elapsed > want + 100:
            return False
    match = re.fullmatch(rb"R3ATH3_SCHED_SLEEP ok count=3 idle_runs=([0-9]+)", lines[4])
    return bool(match) and int(match.group(1)) > 0

def judge_sched_full(text):
    return text == (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_SCHED_FULL ok spawned=6 error=full after_reap=2\n"
    )

def judge_sched_exit(text):
    return text == (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_SCHED_EXIT ok rounds=2 spawned=12 ran=12 free=6 heap_delta=0\n"
    )

def judge_sched_reap(text):
    return text == (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_SCHED_REAP waiters=2 reaped_by=W1 w2_reaped=no free=6\n"
        b"R3ATH3_SCHED_REAP stale_gen=kept same_gen=reaped\n"
        b"R3ATH3_SCHED_REAP ok\n"
    )

def judge_gate_core(text):
    return text == (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_GATE_CORE step=1 refused=no-badge\n"
        b"R3ATH3_GATE_CORE step=2 run=read\n"
        b"R3ATH3_GATE_CORE step=3 refused=no-permission\n"
        b"R3ATH3_GATE_CORE step=4 pending rq=1\n"
        b"R3ATH3_GATE_CORE step=5 refused=busy\n"
        b"R3ATH3_GATE_CORE step=6 wrong=no-pending approved rq=1 action=spawn-nap\n"
        b"R3ATH3_GATE_CORE step=7 replay=no-pending\n"
        b"R3ATH3_GATE_CORE step=8 denied rq=2\n"
        b"R3ATH3_GATE_CORE step=9 cancelled rq=3 then=no-pending next=no-permission\n"
        b"R3ATH3_GATE_CORE step=10 run=read\n"
        b"R3ATH3_GATE_CORE step=11 expired rq=4 at=130\n"
        b"R3ATH3_GATE_CORE step=12 recheck=cancelled rq=5\n"
        b"R3ATH3_GATE_CORE step=13 removed refused=no-badge full=e\n"
        b"R3ATH3_GATE_CORE step=14 parse=ok,syntax,too-long,verb,arg,syntax,syntax\n"
        b"R3ATH3_GATE_CORE ok steps=14\n"
    )

def judge_gate_expire(text):
    lines = _lines(text)
    if len(lines) != 2 or lines[0] != b"R3ATH3_STAGE rust_main":
        return False
    match = re.fullmatch(
        rb"R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=([0-9]+) early_polls=([0-9]+)",
        lines[1],
    )
    if not match:
        return False
    waited = int(match.group(1))
    early = int(match.group(2))
    return 50 <= waited <= 150 and early >= 1

BASIS = 0xcbf29ce484222325
PRIME = 0x100000001b3
MASK = (1 << 64) - 1
LOG_HEADER = re.compile(r"^#([0-9]+) t=([0-9]+) (.+) link=([0-9a-f]{16})$")
LOG_SUMMARY = re.compile(r"^log entries=([0-9]+) of 128 lost=[0-9]+ head=([0-9a-f]{16})$")
REPLAY_SUMMARY = re.compile(r"^replay ok entries=([0-9]+) lost=[0-9]+ head=([0-9a-f]{16})$")
EXP_HEADER = re.compile(r"^#([0-9]+) t=T (.+) link=L$")
EXP_LOG = re.compile(r"^(log entries=([0-9]+) of 128 lost=[0-9]+ head=)H$")
EXP_REPLAY = re.compile(r"^(replay ok entries=([0-9]+) lost=[0-9]+ head=)H$")

def fnv_link(prev, n, tick, tag, body):
    digest = prev
    data = (
        n.to_bytes(4, "little")
        + tick.to_bytes(8, "little")
        + bytes([len(tag)])
        + tag
        + bytes([len(body)])
        + body
    )
    for byte in data:
        digest = ((digest ^ byte) * PRIME) & MASK
    return digest

def _ascii_lines(raw):
    ends = raw.endswith(b"\n")
    text = raw.decode("ascii")
    lines = text.split("\n")
    if ends:
        if lines[-1] != "":
            return None
        lines = lines[:-1]
    return lines, ends

def judge_log(actual, expected):
    try:
        parsed = _ascii_lines(actual)
        expected_text = expected.decode("ascii")
    except UnicodeDecodeError:
        return False
    if parsed is None:
        return False
    if b"R3ATH3_PANIC" in actual or b"R3ATH3_BOOT_OK" in actual:
        return False
    lines, ends = parsed
    entries = {}
    heads = []
    compared = []
    index = 0
    while index < len(lines):
        header = LOG_HEADER.fullmatch(lines[index])
        if header:
            if index + 1 >= len(lines) or not lines[index + 1].startswith("  "):
                return False
            body = lines[index + 1][2:]
            number = int(header.group(1))
            tick = int(header.group(2))
            tag = header.group(3)
            link = int(header.group(4), 16)
            record = (tick, tag, body, link)
            if number in entries and entries[number] != record:
                return False
            entries[number] = record
            compared.append("#%d t=T %s link=L" % (number, tag))
            compared.append(lines[index + 1])
            index += 2
            continue
        summary = LOG_SUMMARY.fullmatch(lines[index]) or REPLAY_SUMMARY.fullmatch(lines[index])
        if summary:
            count = int(summary.group(1))
            head = int(summary.group(2), 16)
            heads.append((count, head))
            compared.append(lines[index][:summary.start(2)] + "H" + lines[index][summary.end(2):])
            index += 1
            continue
        compared.append(lines[index])
        index += 1
    links = {}
    if entries:
        max_n = max(entries)
        if set(entries) != set(range(1, max_n + 1)):
            return False
        prev = BASIS
        for number in range(1, max_n + 1):
            tick, tag, body, link = entries[number]
            if number > 1 and tick < entries[number - 1][0]:
                return False
            expect = fnv_link(prev, number, tick, tag.encode("ascii"), body.encode("ascii"))
            if expect != link:
                return False
            links[number] = expect
            prev = expect
    else:
        max_n = 0
    for count, head in heads:
        if count == 0:
            if head != BASIS:
                return False
        elif count > max_n or head != links[count]:
            return False
    rendered = "\n".join(compared) + ("\n" if ends else "")
    return rendered == expected_text

def fill_log(expected):
    parsed = _ascii_lines(expected)
    if parsed is None:
        raise ValueError("expected log is not ascii text")
    lines, ends = parsed
    info = {}
    index = 0
    while index < len(lines):
        header = EXP_HEADER.fullmatch(lines[index])
        if header:
            number = int(header.group(1))
            tag = header.group(2)
            body = lines[index + 1][2:]
            if number in info and info[number] != (tag, body):
                raise ValueError("inconsistent entry")
            info[number] = (tag, body)
            index += 2
            continue
        index += 1
    max_n = max(info) if info else 0
    prev = BASIS
    chain = {}
    ticks = {}
    for number in range(1, max_n + 1):
        tag, body = info[number]
        ticks[number] = 10 * number
        link = fnv_link(prev, number, ticks[number], tag.encode("ascii"), body.encode("ascii"))
        chain[number] = link
        prev = link
    out = []
    index = 0
    while index < len(lines):
        header = EXP_HEADER.fullmatch(lines[index])
        if header:
            number = int(header.group(1))
            out.append("#%d t=%d %s link=%016x" % (number, ticks[number], header.group(2), chain[number]))
            out.append(lines[index + 1])
            index += 2
            continue
        summary = EXP_LOG.fullmatch(lines[index]) or EXP_REPLAY.fullmatch(lines[index])
        if summary:
            count = int(summary.group(2))
            head = BASIS if count == 0 else chain[count]
            out.append("%s%016x" % (summary.group(1), head))
            index += 1
            continue
        out.append(lines[index])
        index += 1
    return ("\n".join(out) + ("\n" if ends else "")).encode("ascii")

EXP_BOOT_LIST = re.compile(r"^(boot [0-9]+ entries=([0-9]+) lost=[0-9]+ head=)H( chain=\S+ prev=\S+)$")
EXP_BOOT_OK = re.compile(r"^(boot [0-9]+ ok entries=([0-9]+) lost=[0-9]+ head=)H$")
BOOT_LIST = re.compile(
    r"^boot ([0-9]+) entries=([0-9]+) lost=[0-9]+ head=([0-9a-f]{16}) "
    r"chain=(?:ok|#[0-9]+) prev=(?:ok|bad|-)$"
)
BOOT_OK = re.compile(r"^boot ([0-9]+) ok entries=([0-9]+) lost=[0-9]+ head=([0-9a-f]{16})$")

def mask_line(line):
    line = re.sub(r"\bt=[0-9]+", "t=T", line)
    line = re.sub(r"\blink=[0-9a-f]{16}", "link=L", line)
    line = re.sub(r"\bhead=[0-9a-f]{16}", "head=H", line)
    return line

def mask_store_line(line):
    # Same rule as judge_log: only header and summary lines. A body or a
    # command line keeps t=, link=, and head= as written.
    if (
        LOG_HEADER.fullmatch(line)
        or LOG_SUMMARY.fullmatch(line)
        or REPLAY_SUMMARY.fullmatch(line)
        or BOOT_LIST.fullmatch(line)
        or BOOT_OK.fullmatch(line)
    ):
        return mask_line(line)
    return line

def parse_groups(lines):
    groups = {}
    current = None
    index = 0
    while index < len(lines):
        command = re.fullmatch(r"r3> (.*)", lines[index])
        if command:
            cmd = command.group(1)
            boot = re.fullmatch(r"boots ([0-9]+)", cmd)
            if boot:
                current = "s" + boot.group(1)
            elif cmd == "log" or cmd == "replay" or cmd.startswith("replay "):
                current = "live"
            else:
                current = None
            index += 1
            continue
        header = LOG_HEADER.fullmatch(lines[index])
        if header:
            if current is None or index + 1 >= len(lines) or not lines[index + 1].startswith("  "):
                return None
            number = int(header.group(1))
            record = (
                int(header.group(2)),
                header.group(3),
                lines[index + 1][2:],
                int(header.group(4), 16),
            )
            bucket = groups.setdefault(current, {})
            if number in bucket and bucket[number] != record:
                return None
            bucket[number] = record
            index += 2
            continue
        index += 1
    return groups

def check_chain(entries):
    if not entries:
        return True, {}
    numbers = sorted(entries)
    start, end = numbers[0], numbers[-1]
    if numbers != list(range(start, end + 1)):
        return False, {}
    links = {}
    prev = BASIS if start == 1 else None
    for number in range(start, end + 1):
        tick, tag, body, link = entries[number]
        if number > start and tick < entries[number - 1][0]:
            return False, {}
        if prev is None:
            links[number] = link
            prev = link
            continue
        expect = fnv_link(prev, number, tick, tag.encode("ascii"), body.encode("ascii"))
        if expect != link:
            return False, {}
        links[number] = expect
        prev = expect
    return True, links

def judge_store(actual, expected, alias=None, ref=None):
    if b"R3ATH3_PANIC" in actual or b"R3ATH3_BOOT_OK" in actual:
        return False
    try:
        parsed = _ascii_lines(actual)
        expected_text = expected.decode("ascii")
    except UnicodeDecodeError:
        return False
    if parsed is None:
        return False
    lines, ends = parsed
    rendered = "\n".join(mask_store_line(line) for line in lines) + ("\n" if ends else "")
    if rendered != expected_text:
        return False
    groups = parse_groups(lines)
    if groups is None:
        return False
    link_maps = {}
    for name, entries in groups.items():
        ok, links = check_chain(entries)
        if not ok:
            return False
        link_maps[name] = links
    for line in lines:
        listed = BOOT_LIST.fullmatch(line)
        if listed:
            count = int(listed.group(2))
            head = int(listed.group(3), 16)
            group = link_maps.get("s" + listed.group(1), {})
            if count == 0:
                if head != BASIS:
                    return False
            elif count in group and head != group[count]:
                return False
            continue
        ok_line = BOOT_OK.fullmatch(line)
        if ok_line:
            name = "s" + ok_line.group(1)
            head = int(ok_line.group(3), 16)
            entries = groups.get(name, {})
            if not entries:
                if head != BASIS:
                    return False
            elif head != link_maps[name][max(entries)]:
                return False
            continue
        replay = REPLAY_SUMMARY.fullmatch(line)
        if replay:
            head = int(replay.group(2), 16)
            entries = groups.get("live", {})
            if not entries:
                if head != BASIS:
                    return False
            elif head != link_maps["live"][max(entries)]:
                return False
            continue
        summary = LOG_SUMMARY.fullmatch(line)
        if summary:
            count = int(summary.group(1))
            head = int(summary.group(2), 16)
            links = link_maps.get("live", {})
            if count == 0:
                if head != BASIS:
                    return False
            elif count in links and head != links[count]:
                return False
    if alias == "s1=live":
        left = groups.get("s1", {})
        right = groups.get("live", {})
        for number in set(left) & set(right):
            if left[number] != right[number]:
                return False
    if ref:
        ref_parsed = _ascii_lines(ref)
        if ref_parsed is None:
            return False
        ref_groups = parse_groups(ref_parsed[0])
        if ref_groups is None:
            return False
        left = groups.get("s1", {})
        right = ref_groups.get("live", {})
        for number in set(left) & set(right):
            if left[number] != right[number]:
                return False
    return True

def fill_store(expected):
    parsed = _ascii_lines(expected)
    if parsed is None:
        raise ValueError("expected store log is not ascii text")
    lines, ends = parsed
    info = {}
    index = 0
    while index < len(lines):
        header = EXP_HEADER.fullmatch(lines[index])
        if header:
            number = int(header.group(1))
            if index + 1 >= len(lines) or not lines[index + 1].startswith("  "):
                raise ValueError("entry missing body")
            body = lines[index + 1][2:]
            pair = (header.group(2), body)
            if number in info and info[number] != pair:
                raise ValueError("inconsistent entry")
            info[number] = pair
            index += 2
            continue
        index += 1
    max_n = max(info) if info else 0
    prev = BASIS
    chain = {}
    ticks = {}
    for number in range(1, max_n + 1):
        tag, body = info[number]
        ticks[number] = 10 * number
        link = fnv_link(prev, number, ticks[number], tag.encode("ascii"), body.encode("ascii"))
        chain[number] = link
        prev = link

    def head_for(count):
        count = int(count)
        return BASIS if count == 0 else chain[count]

    out = []
    index = 0
    while index < len(lines):
        header = EXP_HEADER.fullmatch(lines[index])
        if header:
            number = int(header.group(1))
            out.append("#%d t=%d %s link=%016x" % (number, ticks[number], header.group(2), chain[number]))
            out.append(lines[index + 1])
            index += 2
            continue
        summary = EXP_LOG.fullmatch(lines[index]) or EXP_REPLAY.fullmatch(lines[index])
        if summary:
            out.append("%s%016x" % (summary.group(1), head_for(summary.group(2))))
            index += 1
            continue
        boot_list = EXP_BOOT_LIST.fullmatch(lines[index])
        if boot_list:
            out.append("%s%016x%s" % (boot_list.group(1), head_for(boot_list.group(2)), boot_list.group(3)))
            index += 1
            continue
        boot_ok = EXP_BOOT_OK.fullmatch(lines[index])
        if boot_ok:
            out.append("%s%016x" % (boot_ok.group(1), head_for(boot_ok.group(2))))
            index += 1
            continue
        out.append(lines[index])
        index += 1
    return ("\n".join(out) + ("\n" if ends else "")).encode("ascii")

def store_core_log():
    return (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_STORE_CORE step=1 blank formatted=yes blocks=512 slots=15 this=1\n"
        b"R3ATH3_STORE_CORE step=2 persisted=6 boot1 entries=6 lost=1 head=68712f5597559a1a chain=ok\n"
        b"R3ATH3_STORE_CORE step=3 remount formatted=no this=2 used=2 prev=ok\n"
        b"R3ATH3_STORE_CORE step=4 saved=1 note=hello n2=gone\n"
        b"R3ATH3_STORE_CORE step=5 tamper boot1 entries=2 head=4d9b4cc92b89320a broken=3 boot2 prev=bad restored=ok\n"
        b"R3ATH3_STORE_CORE step=6 badcheck=superblock badversion=version restored=ok\n"
        b"R3ATH3_STORE_CORE step=7 short=size small=small notblank=not-blank lba0=zero\n"
        b"R3ATH3_STORE_CORE step=8 opened=15 sixteenth=none used=15\n"
        b"R3ATH3_STORE_CORE step=9 writefail saved_upto=1 failed=yes writes_after=0\n"
        b"R3ATH3_STORE_CORE step=10 encode=128 decode=1,10,human last=ok,help link=ebb95e48118d0282\n"
        b"R3ATH3_STORE_CORE ok steps=10\n"
    )

def judge_store_core(text):
    return text == store_core_log()
def recorder_core_log():
    return (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_RECORDER_CORE step=1 len=0 lost=0 head=cbf29ce484222325 verify=ok\n"
        b"R3ATH3_RECORDER_CORE step=2 n=1 link=ebb95e48118d0282\n"
        b"R3ATH3_RECORDER_CORE step=3 n=2 link=4d9b4cc92b89320a verify=ok\n"
        b"R3ATH3_RECORDER_CORE step=4 len=4 agent_before=yes agent_after=no\n"
        b"R3ATH3_RECORDER_CORE step=5 len=6 full=yes lost=1 head=68712f5597559a1a\n"
        b"R3ATH3_RECORDER_CORE step=6 bad=4 noted=1 len=0 lost=5\n"
        b"R3ATH3_RECORDER_CORE step=7 tamper=3 verify=broken at=3 upto2=ok\n"
        b"R3ATH3_RECORDER_CORE step=8 get0=none get7=none get1=10,human last=ok,help\n"
        b"R3ATH3_RECORDER_CORE step=9 card=selftest frames   FAIL no free frame\n"
        b"R3ATH3_RECORDER_CORE step=10 card=selftest: 7 of 8 ok, 1 failed\n"
        b"R3ATH3_RECORDER_CORE step=11 expired=ai1 halt seq=1 rq=4 denied reason=timeout|failed=ai1 spawn-spin seq=7 rq=5 failed reason=full\n"
        b"R3ATH3_RECORDER_CORE ok steps=11\n"
    )

def judge_recorder_core(text):
    return text == recorder_core_log()

def nvme_core_log():
    return (
        b"R3ATH3_STAGE rust_main\n"
        b"R3ATH3_NVME_CORE step=1 found class=010802 bar0=30000000\n"
        b"R3ATH3_NVME_CORE step=2 ready lba_bytes=512 blocks=4096\n"
        b"R3ATH3_NVME_CORE step=3 read lba=0 zero=yes\n"
        b"R3ATH3_NVME_CORE step=4 write lba=7 readback=same\n"
        b"R3ATH3_NVME_CORE step=5 write lba=4095 readback=same\n"
        b"R3ATH3_NVME_CORE step=6 lba=4096 driver=range device=sct0-sc80\n"
        b"R3ATH3_NVME_CORE step=7 flush=ok\n"
        b"R3ATH3_NVME_CORE ok steps=7\n"
    )

def judge_nvme_core(text):
    return text == nvme_core_log()

def nvme_image_ok(data):
    if len(data) != 2097152:
        return False
    expect = bytearray(2097152)
    for lba in (7, 4095):
        off = lba * 512
        for index in range(512):
            expect[off + index] = (index * 7 + lba) & 0xFF
    return data == bytes(expect)

def same_sha256(left, right):
    import hashlib
    return hashlib.sha256(left).digest() == hashlib.sha256(right).digest()
PY
}

load_harness_py() {
  python3 - "$OUT/harness_py.py" "$@" <<'PY'
import importlib.util, sys
path = sys.argv[1]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
name = sys.argv[2]
text = open(sys.argv[3], "rb").read()
fn = getattr(mod, name)
sys.exit(0 if fn(text) else 1)
PY
}

judge_sched_coop() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_COOP_STATUS" ]] || return 1
  load_harness_py judge_sched_coop "$file"
}

judge_sched_preempt() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_PREEMPT_STATUS" ]] || return 1
  load_harness_py judge_sched_preempt "$file"
}

judge_sched_sleep() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_SLEEP_STATUS" ]] || return 1
  load_harness_py judge_sched_sleep "$file"
}

judge_sched_full() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_FULL_STATUS" ]] || return 1
  load_harness_py judge_sched_full "$file"
}

judge_sched_exit() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_EXIT_STATUS" ]] || return 1
  load_harness_py judge_sched_exit "$file"
}

judge_sched_reap() {
  local status="$1" file="$2"
  [[ "$status" -eq "$SCHED_REAP_STATUS" ]] || return 1
  load_harness_py judge_sched_reap "$file"
}

judge_gate_core() {
  local status="$1" file="$2"
  [[ "$status" -eq "$GATE_CORE_STATUS" ]] || return 1
  load_harness_py judge_gate_core "$file"
}

judge_gate_expire() {
  local status="$1" file="$2"
  [[ "$status" -eq "$GATE_EXPIRE_STATUS" ]] || return 1
  load_harness_py judge_gate_expire "$file"
}

judge_recorder_core() {
  local status="$1" file="$2"
  [[ "$status" -eq "$RECORDER_CORE_STATUS" ]] || return 1
  load_harness_py judge_recorder_core "$file"
}

judge_store_core() {
  local status="$1" file="$2"
  [[ "$status" -eq "$STORE_CORE_STATUS" ]] || return 1
  load_harness_py judge_store_core "$file"
}

judge_nvme_core() {
  local status="$1" file="$2"
  [[ "$status" -eq "$NVME_CORE_STATUS" ]] || return 1
  load_harness_py judge_nvme_core "$file"
}

# Status must equal want. alias is "s1=live" or empty. ref is a log path or empty.
judge_shell_store() {
  local status="$1" file="$2" expected="$3" want="$4" alias="$5" ref="$6"
  [[ "$status" -eq "$want" ]] || return 1
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  python3 - "$OUT/harness_py.py" "$file" "$expected" "$alias" "$ref" <<'PY'
import importlib.util, sys
path, actual, expected, alias, ref = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
alias = None if alias == "" else alias
ref_bytes = None if ref == "" else open(ref, "rb").read()
ok = mod.judge_store(
    open(actual, "rb").read(),
    open(expected, "rb").read(),
    alias,
    ref_bytes,
)
sys.exit(0 if ok else 1)
PY
}

# Status must equal want. Masks t=, link=, and head= on header and summary
# lines, then recomputes every masked value from the transcript.
judge_shell_log() {
  local status="$1" file="$2" expected="$3" want="$4"
  [[ "$status" -eq "$want" ]] || return 1
  if grep -F -q "R3ATH3_PANIC" "$file"; then
    return 1
  fi
  if grep -F -q "$MARKER" "$file"; then
    return 1
  fi
  python3 - "$OUT/harness_py.py" "$file" "$expected" <<'PY'
import importlib.util, sys
path, actual, expected = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
ok = mod.judge_log(open(actual, "rb").read(), open(expected, "rb").read())
sys.exit(0 if ok else 1)
PY
}

judge_exact() {
  local status="$1" file="$2" expected_status="$3" expected_file="$4"
  [[ "$status" -eq "$expected_status" ]] || return 1
  cmp -s "$file" "$expected_file"
}

judge_wasm_core() {
  local status="$1" file="$2"
  judge_exact "$status" "$file" "$WASM_CORE_STATUS" "$OUT/task/wasm-core.expected"
}

# Kernel log, native replay, and the committed golden, byte for byte.
judge_wasm_glue() {
  local status="$1" kernel="$2" replay="$3" golden="$4"
  [[ "$status" -eq "$WASM_GLUE_STATUS" ]] || return 1
  cmp -s "$kernel" "$replay" || return 1
  cmp -s "$kernel" "$golden" || return 1
}

judge_app_screen() {
  local status="$1" serial="$2" expected_serial="$3" vga="$4" expected_vga="$5"
  judge_shell_screen "$status" "$serial" "$expected_serial" "$vga" "$expected_vga"
}

selftest_harness() {
  local dir="$OUT/selftest"
  mkdir -p "$dir"
  printf '%s\n' "$MARKER" >"$dir/good.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" >"$dir/missing.log"
  printf '%s\n' "R3ATH3_PANIC message=injected boot fault" >"$dir/panic.log"
  printf '%s\n' "R3ATH3_HANG" >"$dir/hang.log"
  # Panic text is present so the status and panic checks pass. The marker
  # on the next line is what must cause rejection.
  printf '%s\n' "R3ATH3_PANIC message=injected boot fault" "$MARKER" >"$dir/panic-with-marker.log"

  local ok=0
  judge_boot_ok "$PASS_STATUS" "$dir/good.log" || ok=1
  if judge_boot_ok "$PASS_STATUS" "$dir/missing.log"; then ok=1; fi
  if judge_boot_ok 0 "$dir/good.log"; then ok=1; fi
  judge_panic "$FAIL_STATUS" "$dir/panic.log" || ok=1
  if judge_panic "$PASS_STATUS" "$dir/panic.log"; then ok=1; fi
  if judge_panic "$FAIL_STATUS" "$dir/panic-with-marker.log"; then ok=1; fi
  judge_hang "$TIMEOUT_STATUS" "$dir/hang.log" || ok=1
  if judge_hang 0 "$dir/hang.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_FAULT vector=6 name=ud error=none rip=0x1002b0" >"$dir/ud.log"
  printf '' >"$dir/empty.log"
  printf '%s\n' "R3ATH3_FAULT vector=6 name=ud error=none rip=0x1002b0" "$MARKER" >"$dir/ud-with-marker.log"
  printf '%s\n' "R3ATH3_FAULT vector=14 name=pf error=0x0 rip=0x1002b0 cr2=0x40000000" >"$dir/pf.log"
  printf '%s\n' "R3ATH3_FAULT vector=8 name=df error=0x0 rip=0x1002b0" >"$dir/df.log"
  judge_ud "$UD_STATUS" "$dir/ud.log" || ok=1
  judge_pf "$PF_STATUS" "$dir/pf.log" || ok=1
  judge_df "$DF_STATUS" "$dir/df.log" || ok=1
  if judge_ud 0 "$dir/ud.log"; then ok=1; fi
  if judge_ud "$UD_STATUS" "$dir/empty.log"; then ok=1; fi
  if judge_ud "$UD_STATUS" "$dir/ud-with-marker.log"; then ok=1; fi
  if judge_pf "$PF_STATUS" "$dir/empty.log"; then ok=1; fi
  if judge_df 0 "$dir/empty.log"; then ok=1; fi

  printf '%s\n' \
    "R3ATH3_STAGE rust_main" \
    "R3ATH3_SCRIPT_READY" \
    "R3ATH3_OUT hi" \
    "R3ATH3_SCRIPT_OK lines=1" >"$dir/script-ok.log"
  printf '%s\n' \
    "R3ATH3_STAGE rust_main" \
    "R3ATH3_SCRIPT_READY" \
    "R3ATH3_SCRIPT_ERROR line=2 reason=unknown" >"$dir/script-err.log"
  judge_exact "$SCRIPT_OK_STATUS" "$dir/script-ok.log" "$SCRIPT_OK_STATUS" "$dir/script-ok.log" || ok=1
  judge_exact "$SCRIPT_ERR_STATUS" "$dir/script-err.log" "$SCRIPT_ERR_STATUS" "$dir/script-err.log" || ok=1
  if judge_exact 0 "$dir/script-ok.log" "$SCRIPT_OK_STATUS" "$dir/script-ok.log"; then ok=1; fi
  if judge_exact "$SCRIPT_OK_STATUS" "$dir/empty.log" "$SCRIPT_OK_STATUS" "$dir/script-ok.log"; then ok=1; fi
  if judge_exact "$SCRIPT_OK_STATUS" "$dir/script-err.log" "$SCRIPT_OK_STATUS" "$dir/script-ok.log"; then ok=1; fi
  if judge_exact "$SCRIPT_ERR_STATUS" "$dir/empty.log" "$SCRIPT_ERR_STATUS" "$dir/script-err.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_TICKS ok count=300 hz=100 rtc=3" >"$dir/ticks.log"
  printf '%s\n' "R3ATH3_TICKS ok count=300 hz=100 rtc=9" >"$dir/ticks-bad.log"
  judge_ticks "$TICKS_STATUS" "$dir/ticks.log" || ok=1
  if judge_ticks "$TICKS_STATUS" "$dir/ticks-bad.log"; then ok=1; fi
  if judge_ticks 0 "$dir/ticks.log"; then ok=1; fi
  if judge_ticks "$TICKS_STATUS" "$dir/empty.log"; then ok=1; fi
  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_KEYS_READY" "R3ATH3_KEY a" "R3ATH3_KEYS ok text=a drops=0" >"$dir/keys.log"
  judge_exact "$KEYS_STATUS" "$dir/keys.log" "$KEYS_STATUS" "$dir/keys.log" || ok=1
  if judge_exact "$KEYS_STATUS" "$dir/empty.log" "$KEYS_STATUS" "$dir/keys.log"; then ok=1; fi
  printf '%s\n' "R3ATH3_KBD_READY" "R3ATH3_SCRIPT_OK lines=2" >"$dir/kbd.log"
  judge_exact "$KBD_SCRIPT_STATUS" "$dir/kbd.log" "$KBD_SCRIPT_STATUS" "$dir/kbd.log" || ok=1
  if judge_exact "$SCRIPT_OK_STATUS" "$dir/kbd.log" "$KBD_SCRIPT_STATUS" "$dir/kbd.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_HEAP ok size=1048576 used=400 align_max=4096" >"$dir/heap.log"
  printf '%s\n' "R3ATH3_HEAP ok size=1048576 used=0 align_max=4096" >"$dir/heap-zero.log"
  printf '%s\n' "R3ATH3_HEAP ok size=1048576 used=1048577 align_max=4096" >"$dir/heap-big.log"
  printf '%s\n' "R3ATH3_HEAP ok size=1048576 used=400 align_max=8192" >"$dir/heap-bad.log"
  printf '%s\n' "R3ATH3_HEAP_OOM ok chunks=16 used=1048576" >"$dir/heap-oom.log"
  printf '%s\n' "R3ATH3_HEAP_OOM ok chunks=15 used=1048576" >"$dir/heap-oom-bad.log"
  printf '%s\n' "R3ATH3_PANIC location=x.rs:1 message=memory allocation of 2097152 bytes failed" >"$dir/heap-panic.log"
  printf '%s\n' "R3ATH3_PANIC location=x.rs:1 message=injected boot fault" >"$dir/heap-panic-bad.log"
  judge_heap "$HEAP_STATUS" "$dir/heap.log" || ok=1
  if judge_heap 0 "$dir/heap.log"; then ok=1; fi
  if judge_heap "$FAIL_STATUS" "$dir/heap.log"; then ok=1; fi
  if judge_heap "$HEAP_STATUS" "$dir/empty.log"; then ok=1; fi
  if judge_heap "$HEAP_STATUS" "$dir/heap-zero.log"; then ok=1; fi
  if judge_heap "$HEAP_STATUS" "$dir/heap-big.log"; then ok=1; fi
  if judge_heap "$HEAP_STATUS" "$dir/heap-bad.log"; then ok=1; fi
  judge_heap_oom "$HEAP_OOM_STATUS" "$dir/heap-oom.log" || ok=1
  if judge_heap_oom "$HEAP_STATUS" "$dir/heap-oom.log"; then ok=1; fi
  if judge_heap_oom "$HEAP_OOM_STATUS" "$dir/heap-oom-bad.log"; then ok=1; fi
  judge_heap_panic "$FAIL_STATUS" "$dir/heap-panic.log" || ok=1
  if judge_heap_panic "$PASS_STATUS" "$dir/heap-panic.log"; then ok=1; fi
  if judge_heap_panic "$FAIL_STATUS" "$dir/heap-panic-bad.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_FRAMES ok total=15000 free=15000 first=0x300000 kernel_end=0x2a0000" >"$dir/frames.log"
  printf '%s\n' "R3ATH3_FRAMES ok total=15000 free=15000 first=0x100000 kernel_end=0x2a0000" >"$dir/frames-low.log"
  printf '%s\n' "R3ATH3_FRAMES ok total=20000 free=20000 first=0x300000 kernel_end=0x2a0000" >"$dir/frames-big.log"
  printf '%s\n' "R3ATH3_FRAMES_OOM ok allocated=15000 max=0x3ffd000" >"$dir/frames-oom.log"
  printf '%s\n' "R3ATH3_FRAMES_OOM ok allocated=15000 max=0x4000000" >"$dir/frames-oom-high.log"
  printf '%s\n' "R3ATH3_MEM_ERROR reason=startinfo" >"$dir/mem-badinfo.log"
  printf '%s\n' "R3ATH3_MEM_ERROR reason=memmap" >"$dir/mem-badinfo-wrong.log"
  judge_frames "$FRAMES_STATUS" "$dir/frames.log" || ok=1
  if judge_frames 0 "$dir/frames.log"; then ok=1; fi
  if judge_frames "$HEAP_STATUS" "$dir/frames.log"; then ok=1; fi
  if judge_frames "$FRAMES_STATUS" "$dir/empty.log"; then ok=1; fi
  if judge_frames "$FRAMES_STATUS" "$dir/frames-low.log"; then ok=1; fi
  if judge_frames "$FRAMES_STATUS" "$dir/frames-big.log"; then ok=1; fi
  judge_frames_oom "$FRAMES_OOM_STATUS" "$dir/frames-oom.log" || ok=1
  if judge_frames_oom "$FRAMES_STATUS" "$dir/frames-oom.log"; then ok=1; fi
  if judge_frames_oom "$FRAMES_OOM_STATUS" "$dir/frames-oom-high.log"; then ok=1; fi
  judge_mem_badinfo "$MEM_ERROR_STATUS" "$dir/mem-badinfo.log" || ok=1
  if judge_mem_badinfo 0 "$dir/mem-badinfo.log"; then ok=1; fi
  if judge_mem_badinfo "$MEM_ERROR_STATUS" "$dir/mem-badinfo-wrong.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_PANIC message=memory allocation of 2097152 bytes failed" >"$dir/heap-panic-noloc.log"
  if judge_heap_panic "$FAIL_STATUS" "$dir/heap-panic-noloc.log"; then ok=1; fi
  printf '%s\n' "R3ATH3_FRAMES ok total=15000 free=14999 first=0x300000 kernel_end=0x2a0000" >"$dir/frames-unequal.log"
  if judge_frames "$FRAMES_STATUS" "$dir/frames-unequal.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_STAGE rust_main" "r3> exit" "R3ATH3_SHELL_EXIT last=ok" >"$dir/shell-basic.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" "r3> exit" >"$dir/shell-basic-short.log"
  if judge_exact "$SHELL_CMD_STATUS" "$dir/shell-basic.log" "$SHELL_OK_STATUS" "$dir/shell-basic.log"; then ok=1; fi
  if judge_exact "$SHELL_OK_STATUS" "$dir/shell-basic-short.log" "$SHELL_OK_STATUS" "$dir/shell-basic.log"; then ok=1; fi
  printf '%s\n' "R3ATH3_STAGE rust_main" "r3> halt" "R3ATH3_SHELL_HALT" >"$dir/shell-halt.log"
  if judge_exact "$SHELL_OK_STATUS" "$dir/shell-halt.log" "$SHELL_HALT_STATUS" "$dir/shell-halt.log"; then ok=1; fi

  cat >"$dir/shell-mem.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SHELL_READY
r3> mem
mem heap_used=100 heap_size=1048576 frames_total=16000 frames_free=16000
r3> echo x
x
r3> mem
mem heap_used=100 heap_size=1048576 frames_total=16000 frames_free=16000
r3> uptime
uptime ticks=250 hz=100 seconds=2.50
r3> exit
R3ATH3_SHELL_EXIT last=ok
EOF
  cat >"$dir/shell-mem.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SHELL_READY
r3> mem
MEM
r3> echo x
x
r3> mem
MEM
r3> uptime
UPTIME
r3> exit
R3ATH3_SHELL_EXIT last=ok
EOF
  judge_shell_mem "$SHELL_OK_STATUS" "$dir/shell-mem.log" "$dir/shell-mem.expected" || ok=1
  if judge_shell_mem "$SHELL_CMD_STATUS" "$dir/shell-mem.log" "$dir/shell-mem.expected"; then ok=1; fi
  printf '%s\n' "mem heap_used=100 heap_size=1048576 frames_total=15773 frames_free=15773" >"$dir/frames-bound-ok.log"
  printf '%s\n' "mem heap_used=100 heap_size=1048576 frames_total=15748 frames_free=15748" >"$dir/frames-bound-low.log"
  printf '%s\n' "mem heap_used=100 heap_size=1048576 frames_total=15774 frames_free=15774" >"$dir/frames-bound-high.log"
  shell_frames_in_bound "$dir/frames-bound-ok.log" || ok=1
  if shell_frames_in_bound "$dir/frames-bound-low.log"; then ok=1; fi
  if shell_frames_in_bound "$dir/frames-bound-high.log"; then ok=1; fi
  python3 - "$dir/shell-mem.log" "$dir/shell-mem-differ.log" <<'PY'
import sys
src, dst = sys.argv[1:]
lines = open(src).read().splitlines(True)
seen = 0
out = []
for line in lines:
    if line.startswith("mem heap_used="):
        seen += 1
        if seen == 2:
            line = line.replace("heap_used=100", "heap_used=101", 1)
    out.append(line)
open(dst, "w").write("".join(out))
PY
  if judge_shell_mem "$SHELL_OK_STATUS" "$dir/shell-mem-differ.log" "$dir/shell-mem.expected"; then ok=1; fi
  sed 's/heap_used=100/heap_used=5000/g' "$dir/shell-mem.log" >"$dir/shell-mem-big.log"
  if judge_shell_mem "$SHELL_OK_STATUS" "$dir/shell-mem-big.log" "$dir/shell-mem.expected"; then ok=1; fi
  sed 's/frames_free=16000/frames_free=15999/g' "$dir/shell-mem.log" >"$dir/shell-mem-frames.log"
  if judge_shell_mem "$SHELL_OK_STATUS" "$dir/shell-mem-frames.log" "$dir/shell-mem.expected"; then ok=1; fi
  sed 's/seconds=2.50/seconds=3.50/' "$dir/shell-mem.log" >"$dir/shell-mem-uptime.log"
  if judge_shell_mem "$SHELL_OK_STATUS" "$dir/shell-mem-uptime.log" "$dir/shell-mem.expected"; then ok=1; fi

  python3 - "$dir" <<'PY'
import sys
out = sys.argv[1]
cells = [0x0A20] * 2000
def dump(path, values):
    lines = []
    for row in range(25):
        chunk = values[row * 80:(row + 1) * 80]
        lines.append(" ".join(f"{cell:04X}" for cell in chunk))
    open(path, "w", encoding="ascii").write("\n".join(lines) + "\n")
dump(out + "/screen-good.vga", cells)
changed = cells[:]
changed[0] = 0x0A21
dump(out + "/screen-cell.vga", changed)
attr = cells[:]
attr[1] = 0x0720
dump(out + "/screen-attr.vga", attr)
open(out + "/screen-serial.log", "w", encoding="ascii").write("R3ATH3_SHELL_EXIT last=ok\n")
PY
  judge_shell_screen "$SHELL_OK_STATUS" "$dir/screen-serial.log" "$dir/screen-serial.log" \
    "$dir/screen-good.vga" "$dir/screen-good.vga" || ok=1
  if judge_shell_screen "$SHELL_OK_STATUS" "$dir/screen-serial.log" "$dir/screen-serial.log" \
    "$dir/screen-cell.vga" "$dir/screen-good.vga"; then ok=1; fi
  if judge_shell_screen "$SHELL_OK_STATUS" "$dir/screen-serial.log" "$dir/screen-serial.log" \
    "$dir/screen-attr.vga" "$dir/screen-good.vga"; then ok=1; fi

  if ! python3 - "$OUT/harness_py.py" <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("harness_py", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
checks = []
def accept(buf, start, echo):
    checks.append(mod.idle_at_prompt(buf, start, echo))
def reject(buf, start, echo):
    checks.append(not mod.idle_at_prompt(buf, start, echo))
accept(b"banner\nr3> ", 0, b"banner")
accept(b"clear\n\x1b[2J\x1b[Hr3> ", 0, b"clear")
reject(b"banner\nr3> x", 0, b"banner")
reject(b"hi\nxxr3> ", 0, b"hi")
reject(b"\nr3> ", 0, b"help")
if not all(checks):
    sys.exit(1)
keys = []
def kaccept(data, start):
    keys.append(mod.keys_at_prompt(data, start))
def kreject(data, start):
    keys.append(not mod.keys_at_prompt(data, start))
sample = b"x\nr3> "
kaccept(sample, 0)
kreject(sample, len(sample))
kreject(b"xr3> ", 0)
kreject(b"\nr3> x", 0)
kreject(b"xxr3> ", 0)
kreject("x\nr3> ", 0)
if not all(keys):
    sys.exit(1)
PY
  then
    ok=1
  fi

  cat >"$dir/sched-coop.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_COOP task=A step=1
R3ATH3_SCHED_COOP task=B step=1
R3ATH3_SCHED_COOP task=C step=1
R3ATH3_SCHED_COOP task=A step=2
R3ATH3_SCHED_COOP task=B step=2
R3ATH3_SCHED_COOP task=C step=2
R3ATH3_SCHED_COOP task=A step=3
R3ATH3_SCHED_COOP task=B step=3
R3ATH3_SCHED_COOP task=C step=3
R3ATH3_SCHED_COOP ok order=ABCABCABC switches=13
EOF
  cat >"$dir/sched-coop-swap.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_COOP task=B step=1
R3ATH3_SCHED_COOP task=A step=1
R3ATH3_SCHED_COOP task=C step=1
R3ATH3_SCHED_COOP task=A step=2
R3ATH3_SCHED_COOP task=B step=2
R3ATH3_SCHED_COOP task=C step=2
R3ATH3_SCHED_COOP task=A step=3
R3ATH3_SCHED_COOP task=B step=3
R3ATH3_SCHED_COOP task=C step=3
R3ATH3_SCHED_COOP ok order=ABCABCABC switches=13
EOF
  judge_sched_coop "$SCHED_COOP_STATUS" "$dir/sched-coop.log" || ok=1
  if judge_sched_coop "$SCHED_EXIT_STATUS" "$dir/sched-coop.log"; then ok=1; fi
  if judge_sched_coop "$SCHED_COOP_STATUS" "$dir/sched-coop-swap.log"; then ok=1; fi

  cat >"$dir/sched-preempt.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_PREEMPT worker step=1
R3ATH3_SCHED_PREEMPT worker step=2
R3ATH3_SCHED_PREEMPT worker step=3
R3ATH3_SCHED_PREEMPT worker step=4
R3ATH3_SCHED_PREEMPT worker step=5
R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=10 preemptions=6
EOF
  cat >"$dir/sched-preempt-zero-p.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_PREEMPT worker step=1
R3ATH3_SCHED_PREEMPT worker step=2
R3ATH3_SCHED_PREEMPT worker step=3
R3ATH3_SCHED_PREEMPT worker step=4
R3ATH3_SCHED_PREEMPT worker step=5
R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=10 preemptions=0
EOF
  cat >"$dir/sched-preempt-zero-h.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_PREEMPT worker step=1
R3ATH3_SCHED_PREEMPT worker step=2
R3ATH3_SCHED_PREEMPT worker step=3
R3ATH3_SCHED_PREEMPT worker step=4
R3ATH3_SCHED_PREEMPT worker step=5
R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=0 preemptions=6
EOF
  cat >"$dir/sched-preempt-short.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_PREEMPT worker step=1
R3ATH3_SCHED_PREEMPT worker step=2
R3ATH3_SCHED_PREEMPT worker step=3
R3ATH3_SCHED_PREEMPT worker step=4
R3ATH3_SCHED_PREEMPT ok worker_steps=5 hog_spins=10 preemptions=6
EOF
  judge_sched_preempt "$SCHED_PREEMPT_STATUS" "$dir/sched-preempt.log" || ok=1
  if judge_sched_preempt "$SCHED_COOP_STATUS" "$dir/sched-preempt.log"; then ok=1; fi
  if judge_sched_preempt "$SCHED_PREEMPT_STATUS" "$dir/sched-preempt-zero-p.log"; then ok=1; fi
  if judge_sched_preempt "$SCHED_PREEMPT_STATUS" "$dir/sched-preempt-zero-h.log"; then ok=1; fi
  if judge_sched_preempt "$SCHED_PREEMPT_STATUS" "$dir/sched-preempt-short.log"; then ok=1; fi

  cat >"$dir/sched-sleep.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_SLEEP name=S want=10 elapsed=10
R3ATH3_SCHED_SLEEP name=M want=20 elapsed=20
R3ATH3_SCHED_SLEEP name=L want=30 elapsed=30
R3ATH3_SCHED_SLEEP ok count=3 idle_runs=4
EOF
  cat >"$dir/sched-sleep-order.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_SLEEP name=M want=20 elapsed=20
R3ATH3_SCHED_SLEEP name=S want=10 elapsed=10
R3ATH3_SCHED_SLEEP name=L want=30 elapsed=30
R3ATH3_SCHED_SLEEP ok count=3 idle_runs=4
EOF
  cat >"$dir/sched-sleep-early.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_SLEEP name=S want=10 elapsed=9
R3ATH3_SCHED_SLEEP name=M want=20 elapsed=20
R3ATH3_SCHED_SLEEP name=L want=30 elapsed=30
R3ATH3_SCHED_SLEEP ok count=3 idle_runs=4
EOF
  cat >"$dir/sched-sleep-late.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCHED_SLEEP name=S want=10 elapsed=200
R3ATH3_SCHED_SLEEP name=M want=20 elapsed=20
R3ATH3_SCHED_SLEEP name=L want=30 elapsed=30
R3ATH3_SCHED_SLEEP ok count=3 idle_runs=4
EOF
  judge_sched_sleep "$SCHED_SLEEP_STATUS" "$dir/sched-sleep.log" || ok=1
  if judge_sched_sleep "$SCHED_FULL_STATUS" "$dir/sched-sleep.log"; then ok=1; fi
  if judge_sched_sleep "$SCHED_SLEEP_STATUS" "$dir/sched-sleep-order.log"; then ok=1; fi
  if judge_sched_sleep "$SCHED_SLEEP_STATUS" "$dir/sched-sleep-early.log"; then ok=1; fi
  if judge_sched_sleep "$SCHED_SLEEP_STATUS" "$dir/sched-sleep-late.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_SCHED_FULL ok spawned=6 error=full after_reap=2" >"$dir/sched-full.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_SCHED_EXIT ok rounds=2 spawned=12 ran=12 free=6 heap_delta=0" >"$dir/sched-exit.log"
  judge_sched_full "$SCHED_FULL_STATUS" "$dir/sched-full.log" || ok=1
  judge_sched_exit "$SCHED_EXIT_STATUS" "$dir/sched-exit.log" || ok=1
  if judge_sched_full "$SCHED_EXIT_STATUS" "$dir/sched-full.log"; then ok=1; fi
  if judge_sched_exit "$SCHED_FULL_STATUS" "$dir/sched-exit.log"; then ok=1; fi

  printf '%s\n' \
    "R3ATH3_STAGE rust_main" \
    "R3ATH3_SCHED_REAP waiters=2 reaped_by=W1 w2_reaped=no free=6" \
    "R3ATH3_SCHED_REAP stale_gen=kept same_gen=reaped" \
    "R3ATH3_SCHED_REAP ok" >"$dir/sched-reap.log"
  sed '1d' "$dir/sched-reap.log" >"$dir/sched-reap-short.log"
  python3 - "$dir/sched-reap.log" "$dir/sched-reap-swap.log" <<'PY'
import sys
src, dst = sys.argv[1:]
lines = open(src, "rb").read().splitlines(True)
lines[1], lines[2] = lines[2], lines[1]
open(dst, "wb").write(b"".join(lines))
PY
  judge_sched_reap "$SCHED_REAP_STATUS" "$dir/sched-reap.log" || ok=1
  if judge_sched_reap "$GATE_CORE_STATUS" "$dir/sched-reap.log"; then ok=1; fi
  if judge_sched_reap "$SCHED_REAP_STATUS" "$dir/sched-reap-short.log"; then ok=1; fi
  if judge_sched_reap "$SCHED_REAP_STATUS" "$dir/sched-reap-swap.log"; then ok=1; fi

  cat >"$dir/gate-core.log" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_GATE_CORE step=1 refused=no-badge
R3ATH3_GATE_CORE step=2 run=read
R3ATH3_GATE_CORE step=3 refused=no-permission
R3ATH3_GATE_CORE step=4 pending rq=1
R3ATH3_GATE_CORE step=5 refused=busy
R3ATH3_GATE_CORE step=6 wrong=no-pending approved rq=1 action=spawn-nap
R3ATH3_GATE_CORE step=7 replay=no-pending
R3ATH3_GATE_CORE step=8 denied rq=2
R3ATH3_GATE_CORE step=9 cancelled rq=3 then=no-pending next=no-permission
R3ATH3_GATE_CORE step=10 run=read
R3ATH3_GATE_CORE step=11 expired rq=4 at=130
R3ATH3_GATE_CORE step=12 recheck=cancelled rq=5
R3ATH3_GATE_CORE step=13 removed refused=no-badge full=e
R3ATH3_GATE_CORE step=14 parse=ok,syntax,too-long,verb,arg,syntax,syntax
R3ATH3_GATE_CORE ok steps=14
EOF
  sed '3d' "$dir/gate-core.log" >"$dir/gate-core-short.log"
  python3 - "$dir/gate-core.log" "$dir/gate-core-swap.log" <<'PY'
import sys
src, dst = sys.argv[1:]
lines = open(src, "rb").read().splitlines(True)
lines[1], lines[2] = lines[2], lines[1]
open(dst, "wb").write(b"".join(lines))
PY
  judge_gate_core "$GATE_CORE_STATUS" "$dir/gate-core.log" || ok=1
  if judge_gate_core "$GATE_EXPIRE_STATUS" "$dir/gate-core.log"; then ok=1; fi
  if judge_gate_core "$GATE_CORE_STATUS" "$dir/gate-core-short.log"; then ok=1; fi
  if judge_gate_core "$GATE_CORE_STATUS" "$dir/gate-core-swap.log"; then ok=1; fi

  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=50 early_polls=9" >"$dir/gate-expire.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=49 early_polls=9" >"$dir/gate-expire-early.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=200 early_polls=9" >"$dir/gate-expire-late.log"
  printf '%s\n' "R3ATH3_STAGE rust_main" "R3ATH3_GATE_EXPIRE ok rq=1 timeout=50 waited=50 early_polls=0" >"$dir/gate-expire-zero.log"
  judge_gate_expire "$GATE_EXPIRE_STATUS" "$dir/gate-expire.log" || ok=1
  if judge_gate_expire "$GATE_CORE_STATUS" "$dir/gate-expire.log"; then ok=1; fi
  if judge_gate_expire "$GATE_EXPIRE_STATUS" "$dir/gate-expire-early.log"; then ok=1; fi
  if judge_gate_expire "$GATE_EXPIRE_STATUS" "$dir/gate-expire-late.log"; then ok=1; fi
  if judge_gate_expire "$GATE_EXPIRE_STATUS" "$dir/gate-expire-zero.log"; then ok=1; fi

  python3 - "$OUT/harness_py.py" "$dir" <<'PY'
import importlib.util, sys
path, out = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
good = mod.recorder_core_log()
open(out + "/recorder-core.log", "wb").write(good)
lines = good.splitlines(True)
open(out + "/recorder-core-short.log", "wb").write(b"".join(lines[:2] + lines[3:]))
swapped = lines[:]
swapped[1], swapped[2] = swapped[2], swapped[1]
open(out + "/recorder-core-swap.log", "wb").write(b"".join(swapped))
digit = bytearray(good)
spot = digit.find(b"link=ebb95e48118d0282")
digit[spot + len(b"link=")] = ord("0") if digit[spot + len(b"link=")] != ord("0") else ord("1")
open(out + "/recorder-core-digit.log", "wb").write(digit)
PY
  judge_recorder_core "$RECORDER_CORE_STATUS" "$dir/recorder-core.log" || ok=1
  if judge_recorder_core "$GATE_EXPIRE_STATUS" "$dir/recorder-core.log"; then ok=1; fi
  if judge_recorder_core "$RECORDER_CORE_STATUS" "$dir/recorder-core-short.log"; then ok=1; fi
  if judge_recorder_core "$RECORDER_CORE_STATUS" "$dir/recorder-core-swap.log"; then ok=1; fi
  if judge_recorder_core "$RECORDER_CORE_STATUS" "$dir/recorder-core-digit.log"; then ok=1; fi

  python3 - "$OUT/harness_py.py" "$dir" <<'PY'
import importlib.util, sys
path, out = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
good = mod.store_core_log()
open(out + "/store-core.log", "wb").write(good)
lines = good.splitlines(True)
open(out + "/store-core-short.log", "wb").write(b"".join(lines[:4] + lines[5:]))
swapped = lines[:]
swapped[4], swapped[5] = swapped[5], swapped[4]
open(out + "/store-core-swap.log", "wb").write(b"".join(swapped))
digit = bytearray(good)
spot = digit.find(b"head=68712f5597559a1a")
digit[spot + len(b"head=")] = ord("0") if digit[spot + len(b"head=")] != ord("0") else ord("1")
open(out + "/store-core-digit.log", "wb").write(digit)
PY
  judge_store_core "$STORE_CORE_STATUS" "$dir/store-core.log" || ok=1
  if judge_store_core "$RECORDER_CORE_STATUS" "$dir/store-core.log"; then ok=1; fi
  if judge_store_core "$STORE_CORE_STATUS" "$dir/store-core-short.log"; then ok=1; fi
  if judge_store_core "$STORE_CORE_STATUS" "$dir/store-core-swap.log"; then ok=1; fi
  if judge_store_core "$STORE_CORE_STATUS" "$dir/store-core-digit.log"; then ok=1; fi

  if ! write_shell_cases; then
    ok=1
  else
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-ps.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-ps.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-wait.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-wait.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-sched-error.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-error.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-edge.expected" "$SHELL_OVERFLOW_STATUS" "$OUT/task/shell-edge.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-ps.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-wait.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-error.expected"; then ok=1; fi
    judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-approve.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-approve.expected" || ok=1
    judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-noperm.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-noperm.expected" || ok=1
    judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-revoke.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-revoke.expected" || ok=1
    judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-sneak.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-sneak.expected" || ok=1
    judge_exact "$SHELL_HALT_STATUS" "$OUT/task/gate-halt.expected" "$SHELL_HALT_STATUS" "$OUT/task/gate-halt.expected" || ok=1
    judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-usage.expected" || ok=1
    judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-args.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-args.expected" || ok=1
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-approve.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-approve.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-noperm.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-noperm.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-revoke.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-revoke.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-sneak.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-sneak.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-halt.expected" "$SHELL_HALT_STATUS" "$OUT/task/gate-halt.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-sched-args.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-args.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-approve.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-noperm.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/gate-revoke.expected" "$SHELL_OK_STATUS" "$OUT/task/gate-approve.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/gate-sneak.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-args.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-sched-error.expected"; then ok=1; fi
    if judge_exact "$SHELL_HALT_STATUS" "$OUT/task/gate-halt.expected" "$SHELL_HALT_STATUS" "$OUT/task/shell-halt.expected"; then ok=1; fi
    judge_exact "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected" || ok=1
    judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-selftest.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-selftest.expected" || ok=1
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-selftest.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-selftest.expected"; then ok=1; fi
    if judge_exact "$SHELL_OK_STATUS" "$OUT/task/shell-selftest.expected" "$SHELL_OK_STATUS" "$OUT/task/shell-basic.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/gate-usage.expected"; then ok=1; fi
    if ! python3 - "$OUT/harness_py.py" "$OUT/task" <<'PY'
import importlib.util, re, sys
path, task = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

def load(name):
    return open(task + "/" + name, "rb").read()

record = load("log-record.expected")
full = load("log-full.expected")
filled_record = mod.fill_log(record)
filled_full = mod.fill_log(full)
if not mod.judge_log(filled_record, record):
    sys.exit(1)
if not mod.judge_log(filled_full, full):
    sys.exit(1)
open(task + "/log-record.filled", "wb").write(filled_record)
open(task + "/log-full.filled", "wb").write(filled_full)

def must_fail(sample, expected):
    if mod.judge_log(sample, expected):
        sys.exit(1)

text = filled_record.decode("ascii")
link = re.search(r"link=([0-9a-f]{16})", text)
flipped = text.replace("link=" + link.group(1), "link=" + ("0" if link.group(1)[0] != "0" else "1") + link.group(1)[1:], 1)
must_fail(flipped.encode("ascii"), record)
body = text.replace("\n  log\n", "\n  Log\n", 1)
must_fail(body.encode("ascii"), record)
ticks = text.replace("#3 t=30 ", "#3 t=40 ", 1).replace("#4 t=40 ", "#4 t=30 ", 1)
must_fail(ticks.encode("ascii"), record)
replay_head = re.search(r"replay ok entries=20 lost=0 head=([0-9a-f]{16})", text).group(1)
entry1 = re.search(r"#1 t=10 human last=ok link=([0-9a-f]{16})", text).group(1)
other_head = text.replace("replay ok entries=20 lost=0 head=" + replay_head, "replay ok entries=20 lost=0 head=" + entry1, 1)
must_fail(other_head.encode("ascii"), record)
parts = text.split("#11 t=110 ")
different = "#11 t=110 ".join(parts[:2]) + "#11 t=111 " + "#11 t=110 ".join(parts[2:])
must_fail(different.encode("ascii"), record)
lines = filled_record.splitlines(True)
gap = []
skipped = False
for line in lines:
    if not skipped and line.startswith(b"#5 t="):
        skipped = True
        continue
    if skipped:
        skipped = False
        continue
    gap.append(line)
must_fail(b"".join(gap), record)
if mod.judge_log(filled_record, full):
    sys.exit(1)
if mod.judge_log(filled_full, record):
    sys.exit(1)
if mod.judge_log(load("gate-approve.expected"), record):
    sys.exit(1)
PY
    then
      ok=1
    fi
    judge_shell_log "$SHELL_OK_STATUS" "$OUT/task/log-record.filled" "$OUT/task/log-record.expected" "$SHELL_OK_STATUS" || ok=1
    judge_shell_log "$SHELL_OK_STATUS" "$OUT/task/log-full.filled" "$OUT/task/log-full.expected" "$SHELL_OK_STATUS" || ok=1
    if judge_shell_log "$SHELL_CMD_STATUS" "$OUT/task/log-record.filled" "$OUT/task/log-record.expected" "$SHELL_OK_STATUS"; then
      ok=1
    fi
    if ! python3 - "$OUT/harness_py.py" "$OUT" <<'PY'
import importlib.util, re, sys
path, out = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
expected = open(out + "/task/store-ram.expected", "rb").read()
filled = mod.fill_store(expected)
open(out + "/task/store-ram.filled", "wb").write(filled)
if not mod.judge_store(filled, expected, "s1=live"):
    sys.exit(1)
text = filled.decode("ascii")
pos = text.index("r3> boots 1\n")
rel = text.index("link=", pos)
digit = text[rel + 5]
flipped = text[: rel + 5] + ("0" if digit != "0" else "1") + text[rel + 6 :]
if mod.judge_store(flipped.encode("ascii"), expected, "s1=live"):
    sys.exit(1)
rpos = text.index("r3> replay\n")
bpos = text.index("\n  store\n", rpos)
changed = text[: bpos + 3] + "S" + text[bpos + 4 :]
if mod.judge_store(changed.encode("ascii"), expected, "s1=live"):
    sys.exit(1)
lines = text.splitlines()
start = lines.index("r3> boots 1") + 1
end = next(i for i, line in enumerate(lines) if line.startswith("boot 1 ok "))
chunk = []
index = start
while index < end:
    header = re.fullmatch(r"#([0-9]+) t=[0-9]+ (.+) link=[0-9a-f]{16}", lines[index])
    chunk.append((int(header.group(1)), header.group(2), lines[index + 1][2:]))
    index += 2
prev = mod.BASIS
new = []
links = {}
for number, tag, body in chunk:
    tick = 1000 * number
    link = mod.fnv_link(prev, number, tick, tag.encode("ascii"), body.encode("ascii"))
    links[number] = link
    new.append("#%d t=%d %s link=%016x" % (number, tick, tag, link))
    new.append("  " + body)
    prev = link
rewritten = lines[:]
listed = re.sub(r"head=[0-9a-f]{16}", "head=%016x" % links[10], lines[lines.index("r3> boots") + 1])
rewritten[lines.index("r3> boots") + 1] = listed
rewritten[start:end] = new
rewritten[end] = re.sub(r"head=[0-9a-f]{16}", "head=%016x" % links[11], lines[end])
rewritten_b = ("\n".join(rewritten) + "\n").encode("ascii")
if not mod.judge_store(rewritten_b, expected, None):
    sys.exit(1)
if mod.judge_store(rewritten_b, expected, "s1=live"):
    sys.exit(1)
ninth = re.search(r"#9 t=\d+ human last=ok link=([0-9a-f]{16})", text).group(1)
current = re.search(r"boot 1 entries=10 lost=0 head=([0-9a-f]{16})", text).group(1)
replaced = text.replace("head=" + current, "head=" + ninth, 1)
if mod.judge_store(replaced.encode("ascii"), expected, "s1=live"):
    sys.exit(1)
raw = filled.splitlines(True)
gap = []
skip = False
armed = False
for line in raw:
    if line.startswith(b"r3> boots 1"):
        armed = True
    if armed and not skip and line.startswith(b"#5 t="):
        skip = True
        continue
    if skip:
        skip = False
        armed = False
        continue
    gap.append(line)
if mod.judge_store(b"".join(gap), expected, "s1=live"):
    sys.exit(1)
record = open(out + "/task/log-record.expected", "rb").read()
if mod.judge_log(filled, record):
    sys.exit(1)
if mod.judge_store(filled, record, "s1=live"):
    sys.exit(1)
# A command line is not a summary. Masking head= there would hide a mismatch.
loose_actual = filled.replace(
    b"saved note to ram\n", b"saved note to ram head=0123456789abcdef\n", 1
)
loose_expected = expected.replace(b"saved note to ram\n", b"saved note to ram head=H\n", 1)
if mod.judge_store(loose_actual, loose_expected, "s1=live"):
    sys.exit(1)
PY
    then
      ok=1
    fi
    judge_shell_store "$SHELL_OK_STATUS" "$OUT/task/store-ram.filled" "$OUT/task/store-ram.expected" "$SHELL_OK_STATUS" "s1=live" "" || ok=1
    if judge_shell_store "$SHELL_CMD_STATUS" "$OUT/task/store-ram.filled" "$OUT/task/store-ram.expected" "$SHELL_OK_STATUS" "s1=live" ""; then
      ok=1
    fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/store-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/store-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/store-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/shell-usage.expected"; then ok=1; fi
    if judge_exact "$SHELL_CMD_STATUS" "$OUT/task/shell-usage.expected" "$SHELL_CMD_STATUS" "$OUT/task/store-usage.expected"; then ok=1; fi
    python3 - "$OUT/harness_py.py" "$dir" <<'PY'
import importlib.util, sys
path, out = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
good = mod.nvme_core_log()
open(out + "/nvme-core.log", "wb").write(good)
lines = good.splitlines(True)
open(out + "/nvme-core-short.log", "wb").write(b"".join(lines[:6] + lines[7:]))
image = bytearray(2097152)
for lba in (7, 4095):
    off = lba * 512
    for index in range(512):
        image[off + index] = (index * 7 + lba) & 0xFF
open(out + "/nvme-core.img", "wb").write(image)
changed = bytearray(image)
changed[7 * 512] ^= 0x01
open(out + "/nvme-core-lba7.img", "wb").write(changed)
other = bytearray(image)
other[100 * 512] = 1
open(out + "/nvme-core-lba100.img", "wb").write(other)
PY
    judge_nvme_core "$NVME_CORE_STATUS" "$dir/nvme-core.log" || ok=1
    if judge_nvme_core "$STORE_CORE_STATUS" "$dir/nvme-core.log"; then ok=1; fi
    if judge_nvme_core "$NVME_CORE_STATUS" "$dir/nvme-core-short.log"; then ok=1; fi
    if ! python3 - "$OUT/harness_py.py" "$dir" "$OUT" <<'PY'
import importlib.util, re, sys
path, selftest, out = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
good = open(selftest + "/nvme-core.img", "rb").read()
if not mod.nvme_image_ok(good):
    sys.exit(1)
if mod.nvme_image_ok(open(selftest + "/nvme-core-lba7.img", "rb").read()):
    sys.exit(1)
if mod.nvme_image_ok(open(selftest + "/nvme-core-lba100.img", "rb").read()):
    sys.exit(1)
boot1 = mod.fill_store(open(out + "/task/store-reboot.boot1.expected", "rb").read())
boot2 = mod.fill_store(open(out + "/task/store-reboot.boot2.expected", "rb").read())
expected2 = open(out + "/task/store-reboot.boot2.expected", "rb").read()
if not mod.judge_store(boot2, expected2, None, boot1):
    sys.exit(1)
text = boot2.decode("ascii")
start = text.index("r3> boots 1\n")
end = text.index("boot 1 ok ", start)
chunk_lines = text[start:end].splitlines()
records = []
index = 1
while index < len(chunk_lines):
    header = re.fullmatch(r"#([0-9]+) t=[0-9]+ (.+) link=[0-9a-f]{16}", chunk_lines[index])
    if header is None:
        index += 1
        continue
    records.append((int(header.group(1)), header.group(2), chunk_lines[index + 1][2:]))
    index += 2
prev = mod.BASIS
links = {}
new = []
for number, tag, body in records:
    tick = 1000 * number
    link = mod.fnv_link(prev, number, tick, tag.encode("ascii"), body.encode("ascii"))
    links[number] = link
    new.append("#%d t=%d %s link=%016x" % (number, tick, tag, link))
    new.append("  " + body)
    prev = link
lines = text.splitlines()
s1 = lines.index("r3> boots 1")
ok_at = next(i for i, line in enumerate(lines) if line.startswith("boot 1 ok "))
rewritten = lines[:]
rewritten[s1 + 1:ok_at] = new
rewritten[ok_at] = re.sub(r"head=[0-9a-f]{16}", "head=%016x" % links[max(links)], lines[ok_at])
listed = next(i for i, line in enumerate(lines) if line.startswith("boot 1 entries="))
rewritten[listed] = re.sub(r"head=[0-9a-f]{16}", "head=%016x" % links[5], lines[listed])
different = ("\n".join(rewritten) + "\n").encode("ascii")
if mod.judge_store(different, expected2, None, boot1):
    sys.exit(1)
tamper = mod.fill_store(open(out + "/task/store-tamper.boot2.expected", "rb").read())
tamper_expected = open(out + "/task/store-tamper.boot2.expected", "rb").read()
if not mod.judge_store(tamper, tamper_expected):
    sys.exit(1)
zero = tamper.decode("ascii")
spot = zero.index("boot 2 entries=0 ")
head = re.search(r"head=([0-9a-f]{16})", zero[spot:]).group(1)
if head != "cbf29ce484222325":
    sys.exit(1)
bad_head = zero.replace("boot 2 entries=0 lost=0 head=" + head, "boot 2 entries=0 lost=0 head=0000000000000001", 1)
if mod.judge_store(bad_head.encode("ascii"), tamper_expected):
    sys.exit(1)
if mod.judge_store(tamper, expected2):
    sys.exit(1)
blob = b"NOTR3ATH" + bytes(32)
changed = bytearray(blob)
changed[0] ^= 0x01
if not mod.same_sha256(blob, blob):
    sys.exit(1)
if mod.same_sha256(blob, bytes(changed)):
    sys.exit(1)
PY
    then
      ok=1
    fi
    judge_wasm_core "$WASM_CORE_STATUS" "$OUT/task/wasm-core.expected" || ok=1
    sed 's/add=42/add=41/' "$OUT/task/wasm-core.expected" >"$dir/wasm-core-step.log"
    if judge_wasm_core "$WASM_CORE_STATUS" "$dir/wasm-core-step.log"; then ok=1; fi
    grep -v 'ok steps=11' "$OUT/task/wasm-core.expected" >"$dir/wasm-core-nook.log"
    if judge_wasm_core "$WASM_CORE_STATUS" "$dir/wasm-core-nook.log"; then ok=1; fi
    if judge_wasm_core "$NVME_CORE_STATUS" "$OUT/task/wasm-core.expected"; then ok=1; fi
    python3 - "$OUT/task/app-hello.vga" "$dir/app-hello-cell.vga" <<'PY'
import sys
src, dst = sys.argv[1:]
text = open(src, encoding="ascii").read()
text = text.replace("0A20", "0A21", 1)
open(dst, "w", encoding="ascii").write(text)
PY
    judge_app_screen "$SHELL_OK_STATUS" "$OUT/task/app-hello.expected" "$OUT/task/app-hello.expected" "$OUT/task/app-hello.vga" "$OUT/task/app-hello.vga" || ok=1
    if judge_app_screen "$SHELL_OK_STATUS" "$OUT/task/app-hello.expected" "$OUT/task/app-hello.expected" "$dir/app-hello-cell.vga" "$OUT/task/app-hello.vga"; then ok=1; fi
    sed 's/stopped: trap fuel/stopped: trap stack/' "$OUT/task/app-spin.expected" >"$dir/app-spin-stack.log"
    if judge_exact "$SHELL_OK_STATUS" "$dir/app-spin-stack.log" "$SHELL_OK_STATUS" "$OUT/task/app-spin.expected"; then ok=1; fi
    python3 - "$OUT/task/app-gate.expected" "$dir/app-gate-ready.log" <<'PY'
import sys
src, dst = sys.argv[1:]
text = open(src, encoding="ascii").read()
needle = "ai1 says: app spin\n"
insert = needle + "R3ATH3_APP_READY name=spin\n"
if needle not in text:
    raise SystemExit("say line missing")
open(dst, "w", encoding="ascii").write(text.replace(needle, insert, 1))
PY
    if judge_exact "$SHELL_OK_STATUS" "$dir/app-gate-ready.log" "$SHELL_OK_STATUS" "$OUT/task/app-gate.expected"; then ok=1; fi
    local glue_golden="$ROOT/apps/glue-duty/golden/wasm-glue.txt"
    local glue_vga="$ROOT/apps/glue-duty/golden/app-glue.vga"
    cp "$glue_golden" "$dir/wasm-glue-kernel.log"
    cp "$glue_golden" "$dir/wasm-glue-replay.log"
    judge_wasm_glue "$WASM_GLUE_STATUS" "$dir/wasm-glue-kernel.log" "$dir/wasm-glue-replay.log" "$glue_golden" || ok=1
    sed 's/frame=/frame=0/' "$dir/wasm-glue-replay.log" >"$dir/wasm-glue-frame.log"
    if judge_wasm_glue "$WASM_GLUE_STATUS" "$dir/wasm-glue-kernel.log" "$dir/wasm-glue-frame.log" "$glue_golden"; then ok=1; fi
    sed 's/frame=/frame=0/' "$glue_golden" >"$dir/wasm-glue-golden.log"
    if judge_wasm_glue "$WASM_GLUE_STATUS" "$dir/wasm-glue-kernel.log" "$dir/wasm-glue-replay.log" "$dir/wasm-glue-golden.log"; then ok=1; fi
    head -n -1 "$dir/wasm-glue-kernel.log" >"$dir/wasm-glue-short.log"
    if judge_wasm_glue "$WASM_GLUE_STATUS" "$dir/wasm-glue-short.log" "$dir/wasm-glue-replay.log" "$glue_golden"; then ok=1; fi
    python3 - "$glue_vga" "$dir/app-glue-cell.vga" <<'PY'
import sys
src, dst = sys.argv[1:]
text = open(src, encoding="ascii").read()
text = text.replace("0A20", "0A21", 1)
open(dst, "w", encoding="ascii").write(text)
PY
    judge_app_screen "$SHELL_OK_STATUS" "$OUT/task/app-glue.expected" "$OUT/task/app-glue.expected" "$glue_vga" "$glue_vga" || ok=1
    if judge_app_screen "$SHELL_OK_STATUS" "$OUT/task/app-glue.expected" "$OUT/task/app-glue.expected" "$dir/app-glue-cell.vga" "$glue_vga"; then ok=1; fi
  fi

  if [[ "$ok" -eq 0 ]]; then
    log "[PASS] harness-selftest"
  else
    log "[FAIL] harness-selftest"
    FAILURES=$((FAILURES + 1))
  fi
}

build_kernel() {
  local name="$1"
  shift
  local features=("$@")
  local feature_args=()
  if [[ ${#features[@]} -gt 0 ]]; then
    feature_args=(--features "${features[*]}")
  fi
  if ! (cd "$KERNEL_DIR" && cargo build --release --offline "${feature_args[@]}") >"$OUT/$name.build.log" 2>&1; then
    log "[FAIL] build $name"
    cat "$OUT/$name.build.log"
    FAILURES=$((FAILURES + 1))
    return 1
  fi
  cp "$KERNEL_DIR/target/x86_64-unknown-none/release/r3ath3" "$OUT/$name.elf"
  log "[PASS] build $name"
}

# Builds glue_duty.wasm and the native replay before any kernel that bakes
# the module. Two PASS lines, matching the two artifacts.
build_app() {
  if ! "$ROOT/scripts/build-apps.sh" >"$OUT/glue-duty.build.log" 2>&1; then
    log "[FAIL] build glue-duty"
    cat "$OUT/glue-duty.build.log"
    FAILURES=$((FAILURES + 1))
    return 1
  fi
  log "[PASS] build glue-duty-wasm"
  log "[PASS] build glue-duty-replay"
}

# Shell cases. glue=1 runs app-glue and app-spin on the image that baked
# Glue Duty. glue=0 logs those two as skipped: the bare shell has no
# glue-duty line, so app-spin's expected app list would not match, and
# app-glue would have nothing to launch. Every other case still runs.
run_shell_cases() {
  local glue="$1"
  expect_shell shell-basic 12 "$OUT/task/shell-basic.steps" "$OUT/task/shell-basic.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-backspace 12 "$OUT/task/shell-backspace.steps" "$OUT/task/shell-backspace.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-unknown 12 "$OUT/task/shell-unknown.steps" "$OUT/task/shell-unknown.expected" "$SHELL_CMD_STATUS" exact
  expect_shell shell-overflow 20 "$OUT/task/shell-overflow.steps" "$OUT/task/shell-overflow.expected" "$SHELL_OVERFLOW_STATUS" exact
  expect_shell shell-run 12 "$OUT/task/shell-run.steps" "$OUT/task/shell-run.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-run-error 12 "$OUT/task/shell-run-error.steps" "$OUT/task/shell-run-error.expected" "$SHELL_SCRIPT_STATUS" exact
  expect_shell shell-mem 12 "$OUT/task/shell-mem.steps" "$OUT/task/shell-mem.expected" "$SHELL_OK_STATUS" mem
  expect_shell shell-halt 12 "$OUT/task/shell-halt.steps" "$OUT/task/shell-halt.expected" "$SHELL_HALT_STATUS" exact
  expect_shell shell-screen 20 "$OUT/task/shell-screen.steps" "$OUT/task/shell-screen.expected" "$SHELL_OK_STATUS" screen
  expect_shell shell-ps 12 "$OUT/task/shell-ps.steps" "$OUT/task/shell-ps.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-wait 12 "$OUT/task/shell-wait.steps" "$OUT/task/shell-wait.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-sched-error 20 "$OUT/task/shell-sched-error.steps" "$OUT/task/shell-sched-error.expected" "$SHELL_CMD_STATUS" exact
  expect_shell shell-usage 12 "$OUT/task/shell-usage.steps" "$OUT/task/shell-usage.expected" "$SHELL_CMD_STATUS" exact
  expect_shell shell-edge 20 "$OUT/task/shell-edge.steps" "$OUT/task/shell-edge.expected" "$SHELL_OVERFLOW_STATUS" exact
  expect_gate gate-approve 12 "$OUT/task/gate-approve.steps" "$OUT/task/gate-approve.expected" "$SHELL_OK_STATUS"
  expect_gate gate-noperm 12 "$OUT/task/gate-noperm.steps" "$OUT/task/gate-noperm.expected" "$SHELL_OK_STATUS"
  expect_gate gate-revoke 12 "$OUT/task/gate-revoke.steps" "$OUT/task/gate-revoke.expected" "$SHELL_OK_STATUS"
  expect_gate gate-sneak 12 "$OUT/task/gate-sneak.steps" "$OUT/task/gate-sneak.expected" "$SHELL_CMD_STATUS"
  expect_gate gate-halt 12 "$OUT/task/gate-halt.steps" "$OUT/task/gate-halt.expected" "$SHELL_HALT_STATUS"
  expect_gate gate-usage 20 "$OUT/task/gate-usage.steps" "$OUT/task/gate-usage.expected" "$SHELL_CMD_STATUS"
  expect_gate log-record 20 "$OUT/task/log-record.steps" "$OUT/task/log-record.expected" "$SHELL_OK_STATUS" log
  expect_gate log-full 60 "$OUT/task/log-full.steps" "$OUT/task/log-full.expected" "$SHELL_OK_STATUS" log
  expect_shell log-usage 12 "$OUT/task/log-usage.steps" "$OUT/task/log-usage.expected" "$SHELL_CMD_STATUS" exact
  expect_shell shell-selftest 12 "$OUT/task/shell-selftest.steps" "$OUT/task/shell-selftest.expected" "$SHELL_OK_STATUS" exact
  expect_shell shell-sched-args 12 "$OUT/task/shell-sched-args.steps" "$OUT/task/shell-sched-args.expected" "$SHELL_CMD_STATUS" exact
  expect_gate store-ram 20 "$OUT/task/store-ram.steps" "$OUT/task/store-ram.expected" "$SHELL_OK_STATUS" store:s1
  expect_shell store-usage 40 "$OUT/task/store-usage.steps" "$OUT/task/store-usage.expected" "$SHELL_CMD_STATUS" exact
  expect_reboot store-reboot \
    "$OUT/task/store-reboot.boot1.steps" "$OUT/task/store-reboot.boot1.expected" "$SHELL_OK_STATUS" \
    none \
    "$OUT/task/store-reboot.boot2.steps" "$OUT/task/store-reboot.boot2.expected" "$SHELL_OK_STATUS"
  expect_reboot store-tamper \
    "$OUT/task/store-reboot.boot1.steps" "$OUT/task/store-reboot.boot1.expected" "$SHELL_OK_STATUS" \
    flip:4914 \
    "$OUT/task/store-tamper.boot2.steps" "$OUT/task/store-tamper.boot2.expected" "$SHELL_CMD_STATUS"
  expect_baddisk
  expect_shell app-hello 20 "$OUT/task/app-hello.steps" "$OUT/task/app-hello.expected" "$SHELL_OK_STATUS" apphello
  if [[ "$glue" -eq 1 ]]; then
    expect_shell app-glue 20 "$OUT/task/app-glue.steps" "$OUT/task/app-glue.expected" "$SHELL_OK_STATUS" appglue
    expect_shell app-spin 20 "$OUT/task/app-spin.steps" "$OUT/task/app-spin.expected" "$SHELL_OK_STATUS" exact
  else
    log "[SKIP] app-glue (glue-duty did not build)"
    FAILURES=$((FAILURES + 1))
    log "[SKIP] app-spin (app list includes glue-duty)"
    FAILURES=$((FAILURES + 1))
  fi
  expect_shell app-usage 12 "$OUT/task/app-usage.steps" "$OUT/task/app-usage.expected" "$SHELL_CMD_STATUS" exact
  expect_gate app-gate 20 "$OUT/task/app-gate.steps" "$OUT/task/app-gate.expected" "$SHELL_OK_STATUS"
}

run_qemu() {
  local name="$1"
  local seconds="$2"
  local disk="${3:-}"
  local elf="$OUT/$name.elf"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  if [[ ! -f "$elf" ]]; then
    echo "missing elf" >"$log_file"
    echo 127
    return
  fi
  set +e
  # SIGTERM at $seconds. If the guest ignores it, SIGKILL follows 5s later
  # (timeout status 137). A guest that exits on SIGTERM still returns 124.
  # A disk path adds the emulated NVMe controller. No pre-existing case
  # passes one, so those command lines stay as they are.
  if [[ -n "$disk" ]]; then
    timeout --foreground --kill-after=5s "$seconds" qemu-system-x86_64 \
      -machine pc,accel=tcg \
      -cpu qemu64 \
      -m 64M \
      -smp 1 \
      -kernel "$elf" \
      -nic none \
      -display none \
      -vga none \
      -serial stdio \
      -no-reboot \
      -drive "file=${disk},if=none,id=r3disk,format=raw" \
      -device nvme,serial=r3ath3,drive=r3disk \
      -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
      >"$log_file" 2>"$err_file"
  else
    timeout --foreground --kill-after=5s "$seconds" qemu-system-x86_64 \
      -machine pc,accel=tcg \
      -cpu qemu64 \
      -m 64M \
      -smp 1 \
      -kernel "$elf" \
      -nic none \
      -display none \
      -vga none \
      -serial stdio \
      -no-reboot \
      -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
      >"$log_file" 2>"$err_file"
  fi
  local status=$?
  set -e
  echo "$status"
}

write_script_cases() {
  mkdir -p "$OUT/task"
  cat >"$OUT/task/ok.task" <<'EOF'
# comment
set n 2
set k 3
add n k
sub n 1
say "hi"
print n
repeat 2
add n 1
say "x"
end
print n
halt
EOF
  cat >"$OUT/task/ok.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_OUT hi
R3ATH3_OUT n=4
R3ATH3_OUT x
R3ATH3_OUT x
R3ATH3_OUT n=6
R3ATH3_SCRIPT_OK lines=12
EOF
  cat >"$OUT/task/unknown.task" <<'EOF'
set n 1
bogus
EOF
  cat >"$OUT/task/unknown.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=2 reason=unknown
EOF
  cat >"$OUT/task/overflow.task" <<'EOF'
set n 9223372036854775807
add n 1
EOF
  cat >"$OUT/task/overflow.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=2 reason=overflow
EOF
  cat >"$OUT/task/repeat.task" <<'EOF'
repeat 100
say "z"
end
EOF
  cat >"$OUT/task/repeat.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=1 reason=repeat
EOF
  cat >"$OUT/task/repeat-halt.task" <<'EOF'
halt
repeat 100
end
EOF
  cat >"$OUT/task/repeat-halt.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=2 reason=repeat
EOF
  python3 -c 'open("'"$OUT"'/task/limit.task","wb").write(b"\n"*49)'
  cat >"$OUT/task/limit.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=49 reason=limit
EOF
  cat >"$OUT/task/nested.task" <<'EOF'
repeat 1
repeat 1
end
end
EOF
  cat >"$OUT/task/nested.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=2 reason=nested
EOF
  cat >"$OUT/task/unclosed.task" <<'EOF'
repeat 1
say "z"
EOF
  cat >"$OUT/task/unclosed.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=3 reason=unclosed
EOF
  printf '%s\n' 'print n' >"$OUT/task/undefined.task"
  cat >"$OUT/task/undefined.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=1 reason=undefined
EOF
  printf '%s\n' 'set' >"$OUT/task/syntax.task"
  cat >"$OUT/task/syntax.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=1 reason=syntax
EOF
  printf 'say "a\001b"\n' >"$OUT/task/char.task"
  cat >"$OUT/task/char.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=1 reason=char
EOF
  : >"$OUT/task/timeout.task"
  cat >"$OUT/task/timeout.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_SCRIPT_READY
R3ATH3_SCRIPT_ERROR line=1 reason=timeout
EOF
}

run_script_qemu() {
  local name="$1"
  local seconds="$2"
  local script_file="$3"
  local append_run="${4:-1}"
  local elf="$OUT/script.elf"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  if [[ ! -f "$elf" ]]; then
    echo "missing elf" >"$log_file"
    echo 127
    return
  fi
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - "$script_file" "$elf" "$log_file" "$err_file" "$seconds" "$append_run" <<'PY'
import os, select, signal, subprocess, sys, time
script_path, elf, out_log, err_log, seconds, append_run = sys.argv[1:]
payload = open(script_path, "rb").read()
if append_run == "1":
    if not payload.endswith(b"\n"):
        payload += b"\n"
    payload += b"run\n"
err_f = open(err_log, "wb")
proc = subprocess.Popen(
    [
        "qemu-system-x86_64",
        "-machine", "pc,accel=tcg",
        "-cpu", "qemu64",
        "-m", "64M",
        "-smp", "1",
        "-kernel", elf,
        "-nic", "none",
        "-display", "none",
        "-vga", "none",
        "-serial", "stdio",
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=err_f,
)
def stop(_signum, _frame):
    if proc.poll() is None:
        proc.kill()
    sys.exit(124)
signal.signal(signal.SIGTERM, stop)
deadline = time.time() + float(seconds)
buf = b""
sent = False
fd = proc.stdout.fileno()
while True:
    if time.time() > deadline:
        proc.kill()
        proc.wait()
        open(out_log, "wb").write(buf)
        sys.exit(124)
    ready, _, _ = select.select([fd], [], [], 0.05)
    if ready:
        chunk = os.read(fd, 4096)
        if chunk:
            buf += chunk
        elif proc.poll() is not None:
            break
    if (not sent) and b"R3ATH3_SCRIPT_READY\n" in buf:
        view = memoryview(payload)
        try:
            for start in range(0, len(view), 8):
                proc.stdin.write(view[start:start + 8])
                proc.stdin.flush()
                time.sleep(0.001)
        except (BrokenPipeError, OSError):
            pass
        sent = True
    if proc.poll() is not None:
        while True:
            try:
                leftover = os.read(fd, 4096)
            except OSError:
                break
            if not leftover:
                break
            buf += leftover
        break
open(out_log, "wb").write(buf)
code = proc.wait()
sys.exit(code if code is not None else 124)
PY
  local status=$?
  set -e
  echo "$status"
}

expect_script() {
  local name="$1"
  local seconds="$2"
  local script_file="$3"
  local expected="$4"
  local append_run="${5:-1}"
  local want="$SCRIPT_OK_STATUS"
  case "$name" in
    script-ok) want="$SCRIPT_OK_STATUS" ;;
    *) want="$SCRIPT_ERR_STATUS" ;;
  esac
  local status
  status="$(run_script_qemu "$name" "$seconds" "$script_file" "$append_run")"
  if judge_exact "$status" "$OUT/$name.serial.log" "$want" "$expected"; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

write_input_cases() {
  mkdir -p "$OUT/task"
  cat >"$OUT/task/keys.expected" <<'EOF'
R3ATH3_STAGE rust_main
R3ATH3_KEYS_READY
R3ATH3_KEY a
R3ATH3_KEY B
R3ATH3_KEY bs
R3ATH3_KEY C
R3ATH3_KEY enter
R3ATH3_KEYS ok text=aC drops=0
EOF
  python3 - "$OUT/task/kbd.expected" <<'PY'
import sys
lines = ["R3ATH3_STAGE rust_main", "R3ATH3_KBD_READY"]
for phrase in ("set n 1", "print n", "run"):
    buf = ""
    for ch in phrase:
        buf += ch
        lines.append("R3ATH3_ECHO " + buf)
lines.extend(["R3ATH3_OUT n=1", "R3ATH3_SCRIPT_OK lines=2"])
open(sys.argv[1], "w", encoding="utf-8").write("\n".join(lines) + "\n")
PY
}

expect_monitor() {
  local name="$1"
  local seconds="$2"
  local elf="$3"
  local ready="$4"
  local keys="$5"
  local expected="$6"
  local want="$7"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  local sock="$OUT/$name.mon.sock"
  rm -f "$sock" "$log_file"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - "$elf" "$log_file" "$err_file" "$seconds" "$ready" "$keys" "$sock" <<'PY'
import os, select, socket, subprocess, sys, time
elf, out_log, err_log, seconds, ready, keys, sock_path = sys.argv[1:]
# A previous run's transcript must not satisfy the ready wait. QEMU truncates
# the serial file only after it starts, and the first read can happen before that.
try:
    os.remove(out_log)
except FileNotFoundError:
    pass
err_f = open(err_log, "wb")
proc = subprocess.Popen(
    [
        "qemu-system-x86_64", "-machine", "pc,accel=tcg", "-cpu", "qemu64",
        "-m", "64M", "-smp", "1", "-kernel", elf, "-nic", "none",
        "-display", "none", "-vga", "std", "-serial", "file:" + out_log,
        "-monitor", "unix:" + sock_path + ",server,nowait", "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ],
    stdout=subprocess.DEVNULL, stderr=err_f,
)
deadline = time.time() + float(seconds)
marker = ready.encode()
while time.time() < deadline:
    if proc.poll() is not None:
        break
    try:
        data = open(out_log, "rb").read()
    except FileNotFoundError:
        data = b""
    if marker in data:
        break
    time.sleep(0.02)
else:
    proc.kill(); proc.wait(); sys.exit(124)
mon = None
for _ in range(50):
    if os.path.exists(sock_path):
        candidate = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            candidate.connect(sock_path)
            mon = candidate
            break
        except OSError:
            candidate.close()
    time.sleep(0.1)
if mon is None:
    message = "monitor socket did not appear: " + sock_path + "\n"
    sys.stderr.write(message)
    try:
        err_f.write(message.encode())
        err_f.flush()
    except OSError:
        pass
    if proc.poll() is None:
        proc.kill()
        proc.wait()
    sys.exit(1)
mon.settimeout(0.2)
try:
    mon.recv(4096)
except Exception:
    pass
for key in keys.split():
    try:
        mon.sendall(f"sendkey {key}\n".encode())
    except OSError:
        break
    time.sleep(0.03)
code = proc.wait() if proc.poll() is not None else None
end = time.time() + 2
while proc.poll() is None and time.time() < end:
    time.sleep(0.02)
if proc.poll() is None:
    proc.kill()
    code = proc.wait()
else:
    code = proc.wait()
try:
    mon.sendall(b"quit\n")
except OSError:
    pass
mon.close()
sys.exit(code if code is not None else 124)
PY
  local status=$?
  set -e
  if judge_exact "$status" "$log_file" "$want" "$expected"; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$log_file" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$err_file" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

write_shell_cases() {
  python3 - "$OUT" <<'PY'
import os, sys
out = sys.argv[1]
os.makedirs(out + "/task", exist_ok=True)
prefix = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_SHELL_READY\n"
    "R3@TH3 shell. Type help for commands.\n"
    "r3> "
)
help_text = (
    "help        list commands\n"
    "echo TEXT   print TEXT\n"
    "clear       clear the screen\n"
    "uptime      timer ticks and seconds since boot\n"
    "mem         heap and frame use\n"
    "ps          list tasks\n"
    "spawn DEMO  start demo task spin or nap\n"
    "wait ID     wait for a task to exit\n"
    "badges      list AI badges and the waiting request\n"
    "grant A P   give agent A permission P\n"
    "revoke A P  take permission P (or all) from agent A\n"
    "approve N   let gate request N run\n"
    "deny N      refuse gate request N\n"
    "log         newest flight-recorder entries and totals\n"
    "replay [N]  check the record and print it, or entry N\n"
    "selftest    run the feature checks and print a report card\n"
    "store       storage device and what survives a reboot\n"
    "boots [K]   list saved boot records, or print boot K\n"
    "save N TEXT keep TEXT under name N\n"
    "show [N]    list saved names, or the text saved as N\n"
    "forget N    remove the text saved as N\n"
    "app [NAME]  list built-in WASM apps, or start app NAME\n"
    "run SCRIPT  run a task script, lines separated by ;\n"
    "exit, halt  leave with the last status, or stop the machine\n"
)
p = "r3> "
ansi = "\x1b[2J\x1b[H"
exit_ok = "R3ATH3_SHELL_EXIT last=ok\n"

def write_bytes(name, data):
    open(out + "/task/" + name, "wb").write(data.encode("latin1"))

def write_text(name, text):
    open(out + "/task/" + name, "w", encoding="utf-8").write(text)

write_text("shell-basic.steps", "line\nline help\nline echo hello  world\nline clear\nline exit\n")
write_bytes(
    "shell-basic.expected",
    prefix
    + "\n"
    + p
    + "help\n"
    + help_text
    + p
    + "echo hello  world\n"
    + "hello world\n"
    + p
    + "clear\n"
    + ansi
    + p
    + "exit\n"
    + exit_ok,
)

write_text(
    "shell-backspace.steps",
    "keys backspace e c z backspace h o spc h i backspace backspace y o ret\nline exit\n",
)
write_bytes(
    "shell-backspace.expected",
    prefix
    + "ecz"
    + "\x08 \x08"
    + "ho hi"
    + "\x08 \x08"
    + "\x08 \x08"
    + "yo\n"
    + "yo\n"
    + p
    + "exit\n"
    + exit_ok,
)

write_text("shell-unknown.steps", "line foo\nline clear x\nline exit\n")
write_bytes(
    "shell-unknown.expected",
    prefix
    + "foo\n"
    + "error: unknown command: foo\n"
    + p
    + "clear x\n"
    + "error: usage: clear\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=cmd\n",
)

write_text("shell-overflow.steps", "keys " + " ".join(["a"] * 76) + " b c ret\nline exit\n")
write_bytes(
    "shell-overflow.expected",
    prefix
    + ("a" * 76)
    + "\n"
    + "error: line too long (max 76)\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=overflow\n",
)

run1 = 'run set n 2; add n 3; print n; say "hi"; repeat 2; say "x"; end'
run2 = 'run set a 1; halt; say "no"'
write_text(
    "shell-run.steps",
    "line " + run1 + "\nline " + run2 + "\nline echo after\nline exit\n",
)
write_bytes(
    "shell-run.expected",
    prefix
    + run1
    + "\n"
    + "n=5\nhi\nx\nx\n"
    + p
    + run2
    + "\n"
    + p
    + "echo after\n"
    + "after\n"
    + p
    + "exit\n"
    + exit_ok,
)

err1 = 'run set n 1; bogus'
err2 = 'run repeat 1; say "z"'
write_text("shell-run-error.steps", "line " + err1 + "\nline " + err2 + "\nline exit\n")
write_bytes(
    "shell-run-error.expected",
    prefix
    + err1
    + "\n"
    + "error: script line=2 reason=unknown\n"
    + p
    + err2
    + "\n"
    + "error: script line=3 reason=unclosed\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=script\n",
)

write_text("shell-mem.steps", "line mem\nline echo x\nline mem\nline uptime\nline exit\n")
write_bytes(
    "shell-mem.expected",
    prefix + "mem\nMEM\n" + p + "echo x\nx\n" + p + "mem\nMEM\n" + p + "uptime\nUPTIME\n" + p + "exit\n" + exit_ok,
)

write_text("shell-halt.steps", "line foo\nline halt\n")
write_bytes(
    "shell-halt.expected",
    prefix + "foo\nerror: unknown command: foo\n" + p + "halt\nR3ATH3_SHELL_HALT\n",
)

write_text("shell-ps.steps", "line spawn spin\nline ps\nline echo alive\nline exit\n")
write_bytes(
    "shell-ps.expected",
    prefix
    + "spawn spin\n"
    + "spawned id=2 name=spin\n"
    + p
    + "ps\n"
    + "task id=0 name=shell state=running\n"
    + "task id=1 name=idle state=ready\n"
    + "task id=2 name=spin state=ready\n"
    + p
    + "echo alive\n"
    + "alive\n"
    + p
    + "exit\n"
    + exit_ok,
)

write_text("shell-wait.steps", "line spawn nap\nline wait 2\nline ps\nline exit\n")
write_bytes(
    "shell-wait.expected",
    prefix
    + "spawn nap\n"
    + "spawned id=2 name=nap\n"
    + p
    + "wait 2\n"
    + "exited id=2\n"
    + p
    + "ps\n"
    + "task id=0 name=shell state=running\n"
    + "task id=1 name=idle state=ready\n"
    + p
    + "exit\n"
    + exit_ok,
)

sched_err_steps = [
    "line spawn",
    "line spawn foo",
    "line wait 5",
    "line ps x",
] + ["line spawn spin"] * 7 + ["line exit"]
write_text("shell-sched-error.steps", "\n".join(sched_err_steps) + "\n")
sched_err = (
    "spawn\n"
    + "error: usage: spawn DEMO\n"
    + p
    + "spawn foo\n"
    + "error: unknown demo: foo\n"
    + p
    + "wait 5\n"
    + "error: no task 5\n"
    + p
    + "ps x\n"
    + "error: usage: ps\n"
)
for slot in range(2, 8):
    sched_err += p + "spawn spin\n" + "spawned id=%d name=spin\n" % slot
sched_err += (
    p
    + "spawn spin\n"
    + "error: task table full\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=cmd\n"
)
write_bytes("shell-sched-error.expected", prefix + sched_err)

write_text(
    "shell-usage.steps",
    "line run\nline help x\nline uptime x\nline mem x\nline exit x\nline halt x\nline exit\n",
)
write_bytes(
    "shell-usage.expected",
    prefix
    + "run\n"
    + "error: usage: run SCRIPT\n"
    + p
    + "help x\n"
    + "error: usage: help\n"
    + p
    + "uptime x\n"
    + "error: usage: uptime\n"
    + p
    + "mem x\n"
    + "error: usage: mem\n"
    + p
    + "exit x\n"
    + "error: usage: exit\n"
    + p
    + "halt x\n"
    + "error: usage: halt\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=cmd\n",
)

echo_edge = "echo " + ("b" * 71)
if len(echo_edge) != 76:
    raise SystemExit("shell-edge line is not 76 bytes")
write_text(
    "shell-edge.steps",
    "line " + echo_edge + "\nkeys " + " ".join(["a"] * 76) + " b backspace backspace ret\nline exit\n",
)
write_bytes(
    "shell-edge.expected",
    prefix
    + echo_edge
    + "\n"
    + ("b" * 71)
    + "\n"
    + p
    + ("a" * 76)
    + "\x08 \x08\x08 \x08"
    + "\n"
    + "error: line too long (max 76)\n"
    + p
    + "exit\n"
    + "R3ATH3_SHELL_EXIT last=overflow\n",
)

screen_steps = "line clear\n" + ("line\n" * 24) + "line echo vga\nxp\nline exit\n"
write_text("shell-screen.steps", screen_steps)
write_bytes(
    "shell-screen.expected",
    prefix
    + "clear\n"
    + ansi
    + p
    + ("\n" + p) * 24
    + "echo vga\n"
    + "vga\n"
    + p
    + "exit\n"
    + exit_ok,
)

# Model of the VGA console in section 5.2, after the shell-screen steps.
WIDTH, HEIGHT = 80, 25
ATTR = 0x0A00

class Console:
    def __init__(self):
        self.cells = [ATTR | 0x20] * (WIDTH * HEIGHT)
        self.row = 0
        self.col = 0

    def clear(self):
        self.cells = [ATTR | 0x20] * (WIDTH * HEIGHT)
        self.row = 0
        self.col = 0

    def newline(self):
        self.col = 0
        if self.row + 1 == HEIGHT:
            self.cells[: WIDTH * (HEIGHT - 1)] = self.cells[WIDTH:]
            self.cells[WIDTH * (HEIGHT - 1) :] = [ATTR | 0x20] * WIDTH
        else:
            self.row += 1

    def put(self, byte):
        if byte == 10:
            self.newline()
            return
        if self.col == WIDTH:
            self.newline()
        glyph = byte if 0x20 <= byte <= 0x7E else ord("?")
        self.cells[self.row * WIDTH + self.col] = ATTR | glyph
        self.col += 1

    def puts(self, text):
        for byte in text.encode("ascii"):
            self.put(byte)

console = Console()
console.clear()
console.puts("r3> ")
for _ in range(24):
    console.put(10)
    console.puts("r3> ")
console.puts("echo vga")
console.put(10)
console.puts("vga\n")
console.puts("r3> ")

def row_text(row):
    chars = []
    for col in range(WIDTH):
        cell = console.cells[row * WIDTH + col]
        if (cell >> 8) != 0x0A:
            raise SystemExit("attribute drifted")
        chars.append(chr(cell & 0xFF))
    return "".join(chars)

for row in range(22):
    if row_text(row) != "r3> " + (" " * 76):
        raise SystemExit(f"row {row} mismatch")
if row_text(22) != "r3> echo vga" + (" " * 68):
    raise SystemExit("row 22 mismatch")
if row_text(23) != "vga" + (" " * 77):
    raise SystemExit("row 23 mismatch")
if row_text(24) != "r3> " + (" " * 76):
    raise SystemExit("row 24 mismatch")

lines = []
for row in range(HEIGHT):
    chunk = console.cells[row * WIDTH : (row + 1) * WIDTH]
    lines.append(" ".join(f"{cell:04X}" for cell in chunk))
open(out + "/task/shell-screen.vga", "w", encoding="ascii").write("\n".join(lines) + "\n")

nap_explain = "start demo task nap (sleeps 0.2 s, then exits)"
spin_explain = "start demo task spin (runs forever; no way to stop it)"
halt_explain = "stop the machine (ends this session)"
ps_two = (
    "task id=0 name=shell state=running\n"
    "task id=1 name=idle state=ready\n"
)
exit_cmd = "R3ATH3_SHELL_EXIT last=cmd\n"

def pending_block(rq, agent, explain, seq, verb):
    return (
        "\n"
        + "gate: request %d from %s wants to:\n" % (rq, agent)
        + "gate:   %s\n" % explain
        + "gate: type approve %d or deny %d within 60 s\n" % (rq, rq)
        + "R3GATE reply agent=%s seq=%d verb=%s result=pending rq=%d\n" % (agent, seq, verb, rq)
        + p
    )

def refused(agent, human, seq, verb, reason):
    return (
        "\n"
        + "gate: refused %s: %s\n" % (agent, human)
        + "R3GATE reply agent=%s seq=%d verb=%s result=denied reason=%s\n" % (agent, seq, verb, reason)
        + p
    )

def bad_line(reason, agent="-", seq="-", verb="-"):
    return (
        "\n"
        + "gate: ignored a bad request line (%s)\n" % reason
        + "R3GATE reply agent=%s seq=%s verb=%s result=bad reason=%s\n" % (agent, seq, verb, reason)
        + p
    )

def typed(text, output):
    return text + "\n" + output + "\n" + p

write_text(
    "gate-approve.steps",
    "line grant ai1 spawn\nai R3GATE ASK ai1 1 spawn nap\nline ps\nline approve 1\nline wait 2\nline exit\n",
)
write_bytes(
    "gate-approve.expected",
    prefix
    + typed("grant ai1 spawn", "granted ai1 spawn")
    + pending_block(1, "ai1", nap_explain, 1, "spawn")
    + typed("ps", ps_two[:-1])
    + "approve 1\n"
    + "gate: request 1 approved: started task 2 (nap)\n"
    + "R3GATE reply agent=ai1 seq=1 verb=spawn result=done rq=1 id=2\n"
    + p
    + typed("wait 2", "exited id=2")
    + "exit\n"
    + exit_ok,
)

write_text(
    "gate-noperm.steps",
    "ai R3GATE ASK ai1 1 read\n"
    "line grant ai1 read\n"
    "ai R3GATE ASK ai1 2 spawn nap\n"
    "ai R3GATE ASK ai1 3 read\n"
    "line badges\n"
    "line ps\n"
    "line exit\n",
)
write_bytes(
    "gate-noperm.expected",
    prefix
    + refused("ai1", "no badge", 1, "read", "no-badge")
    + typed("grant ai1 read", "granted ai1 read")
    + refused("ai1", "no permission to spawn", 2, "spawn", "no-permission")
    + "\n"
    + "gate: ai1 read the work orders\n"
    + "R3GATE data seq=3 n=1 text=WO-1 check spindle oil level on mill 2\n"
    + "R3GATE data seq=3 n=2 text=WO-2 replace air filter on compressor A\n"
    + "R3GATE data seq=3 n=3 text=WO-3 log hours for weekly maintenance\n"
    + "R3GATE reply agent=ai1 seq=3 verb=read result=done\n"
    + p
    + typed("badges", "badge ai1 may=read\nno pending request")
    + typed("ps", ps_two[:-1])
    + "exit\n"
    + exit_ok,
)

write_text(
    "gate-revoke.steps",
    "line grant ai1 spawn\n"
    "line grant ai1 say\n"
    "ai R3GATE ASK ai1 1 spawn nap\n"
    "line revoke ai1 spawn\n"
    "line approve 1\n"
    "ai R3GATE ASK ai1 2 spawn nap\n"
    "ai R3GATE ASK ai1 3 say still here\n"
    "line revoke ai1 all\n"
    "ai R3GATE ASK ai1 4 say hi\n"
    "line badges\n"
    "line ps\n"
    "line exit\n",
)
write_bytes(
    "gate-revoke.expected",
    prefix
    + typed("grant ai1 spawn", "granted ai1 spawn")
    + typed("grant ai1 say", "granted ai1 say")
    + pending_block(1, "ai1", nap_explain, 1, "spawn")
    + "revoke ai1 spawn\n"
    + "revoked ai1 spawn\n"
    + "gate: request 1 cancelled: ai1 may no longer spawn\n"
    + "R3GATE reply agent=ai1 seq=1 verb=spawn result=denied rq=1 reason=revoked\n"
    + p
    + typed("approve 1", "error: no pending request 1")
    + refused("ai1", "no permission to spawn", 2, "spawn", "no-permission")
    + "\n"
    + "ai1 says: still here\n"
    + "R3GATE reply agent=ai1 seq=3 verb=say result=done\n"
    + p
    + typed("revoke ai1 all", "revoked ai1 all")
    + refused("ai1", "no badge", 4, "say", "no-badge")
    + typed("badges", "no badges\nno pending request")
    + typed("ps", ps_two[:-1])
    + "exit\n"
    + exit_ok,
)

long_say = "R3GATE ASK ai1 2 say " + ("x" * 70)
write_text(
    "gate-sneak.steps",
    "ai halt\n"
    "ai R3GATE ASK ai1 1 approve 1\n"
    "ai " + long_say + "\n"
    "line grant ai1 spawn\n"
    "ai R3GATE ASK ai1 3 spawn spin\n"
    "ai R3GATE ASK ai1 4 spawn nap\n"
    "ai R3GATE ASK ai1 5 halt\n"
    "line approve 2\n"
    "line approve\n"
    "line deny 1\n"
    "line ps\n"
    "line approve 1\n"
    "line exit\n",
)
write_bytes(
    "gate-sneak.expected",
    prefix
    + bad_line("syntax")
    + bad_line("verb", "ai1", "1", "-")
    + bad_line("too-long")
    + typed("grant ai1 spawn", "granted ai1 spawn")
    + pending_block(1, "ai1", spin_explain, 3, "spawn")
    + refused("ai1", "another request is waiting", 4, "spawn", "busy")
    + refused("ai1", "no permission to halt", 5, "halt", "no-permission")
    + typed("approve 2", "error: no pending request 2")
    + typed("approve", "error: usage: approve N")
    + "deny 1\n"
    + "gate: request 1 denied\n"
    + "R3GATE reply agent=ai1 seq=3 verb=spawn result=denied rq=1 reason=human\n"
    + p
    + typed("ps", ps_two[:-1])
    + typed("approve 1", "error: no pending request 1")
    + "exit\n"
    + exit_cmd,
)

write_text(
    "gate-halt.steps",
    "line grant ai1 halt\nai R3GATE ASK ai1 1 halt\nline badges\nline approve 1\n",
)
write_bytes(
    "gate-halt.expected",
    prefix
    + typed("grant ai1 halt", "granted ai1 halt")
    + pending_block(1, "ai1", halt_explain, 1, "halt")
    + typed("badges", "badge ai1 may=halt\npending rq=1 agent=ai1 action=halt")
    + "approve 1\n"
    + "gate: request 1 approved: stopping the machine\n"
    + "R3GATE reply agent=ai1 seq=1 verb=halt result=done rq=1\n"
    + "R3ATH3_SHELL_HALT\n",
)

usage_steps = [
    ("grant", "error: usage: grant AGENT PERM"),
    ("grant ai1", "error: usage: grant AGENT PERM"),
    ("grant ai1 fly", "error: unknown permission: fly"),
    ("grant AI1 read", "error: bad agent name: AI1"),
    ("grant abcdefghi read", "error: bad agent name: abcdefghi"),
    ("grant a read", "granted a read"),
    ("grant b read", "granted b read"),
    ("grant c read", "granted c read"),
    ("grant d read", "granted d read"),
    ("grant e read", "error: badge table full"),
    ("revoke", "error: usage: revoke AGENT PERM"),
    ("revoke zz read", "error: no badge zz"),
    ("revoke a fly", "error: unknown permission: fly"),
    ("approve x", "error: no pending request x"),
    ("deny", "error: usage: deny N"),
    ("badges x", "error: usage: badges"),
]
write_text("gate-usage.steps", "".join("line " + text + "\n" for text, _ in usage_steps) + "line exit\n")
usage_body = "".join(typed(text, output) for text, output in usage_steps)
write_bytes("gate-usage.expected", prefix + usage_body + "exit\n" + exit_cmd)

write_text(
    "shell-sched-args.steps",
    "line wait\nline wait 2 3\nline wait abc\nline wait 99\nline spawn a b\nline exit\n",
)
write_bytes(
    "shell-sched-args.expected",
    prefix
    + typed("wait", "error: usage: wait ID")
    + typed("wait 2 3", "error: usage: wait ID")
    + typed("wait abc", "error: no task abc")
    + typed("wait 99", "error: no task 99")
    + typed("spawn a b", "error: usage: spawn DEMO")
    + "exit\n"
    + exit_cmd,
)

def entry(n, tag, body):
    return "#%d t=T %s link=L\n  %s\n" % (n, tag, body)

log_records = [
    (1, "human last=ok", "log"),
    (2, "gate", "grant ai1 spawn"),
    (3, "human last=ok", "grant ai1 spawn"),
    (4, "gate", "grant ai1 say"),
    (5, "human last=ok", "grant ai1 say"),
    (6, "agent ai1", "say seq=1 done text=hello"),
    (7, "agent ai1", "- seq=2 bad reason=verb"),
    (8, "agent zed", "read seq=3 denied reason=no-badge"),
    (9, "agent ai1", "spawn-nap seq=4 rq=1 pending"),
    (10, "human last=cmd", "approve 9"),
    (11, "gate", "ai1 spawn-nap seq=4 rq=1 done id=2"),
    (12, "human last=ok", "approve 1"),
    (13, "human last=ok", "wait 2"),
    (14, "agent ai1", "spawn-spin seq=5 rq=2 pending"),
    (15, "gate", "ai1 spawn-spin seq=5 rq=2 denied reason=human"),
    (16, "human last=ok", "deny 2"),
    (17, "agent ai1", "spawn-nap seq=6 rq=3 pending"),
    (18, "gate", "revoke ai1 spawn"),
    (19, "gate", "ai1 spawn-nap seq=6 rq=3 denied reason=revoked"),
    (20, "human last=ok", "revoke ai1 spawn"),
    (21, "human last=ok", "replay"),
    (22, "human last=ok", "replay 11"),
]

def entries(nums):
    return "".join(entry(n, log_records[n - 1][1], log_records[n - 1][2]) for n in nums)

write_text(
    "log-record.steps",
    "line log\n"
    "line grant ai1 spawn\n"
    "line grant ai1 say\n"
    "ai R3GATE ASK ai1 1 say hello\n"
    "ai R3GATE ASK ai1 2 log\n"
    "ai R3GATE ASK zed 3 read\n"
    "ai R3GATE ASK ai1 4 spawn nap\n"
    "line approve 9\n"
    "line approve 1\n"
    "line wait 2\n"
    "ai R3GATE ASK ai1 5 spawn spin\n"
    "line deny 2\n"
    "ai R3GATE ASK ai1 6 spawn nap\n"
    "line revoke ai1 spawn\n"
    "line replay\n"
    "line replay 11\n"
    "line log\n"
    "line exit\n",
)
write_bytes(
    "log-record.expected",
    prefix
    + typed("log", "log entries=0 of 128 lost=0 head=H")
    + typed("grant ai1 spawn", "granted ai1 spawn")
    + typed("grant ai1 say", "granted ai1 say")
    + "\n"
    + "ai1 says: hello\n"
    + "R3GATE reply agent=ai1 seq=1 verb=say result=done\n"
    + p
    + bad_line("verb", "ai1", "2", "-")
    + refused("zed", "no badge", 3, "read", "no-badge")
    + pending_block(1, "ai1", nap_explain, 4, "spawn")
    + typed("approve 9", "error: no pending request 9")
    + "approve 1\n"
    + "gate: request 1 approved: started task 2 (nap)\n"
    + "R3GATE reply agent=ai1 seq=4 verb=spawn result=done rq=1 id=2\n"
    + p
    + typed("wait 2", "exited id=2")
    + pending_block(2, "ai1", spin_explain, 5, "spawn")
    + "deny 2\n"
    + "gate: request 2 denied\n"
    + "R3GATE reply agent=ai1 seq=5 verb=spawn result=denied rq=2 reason=human\n"
    + p
    + pending_block(3, "ai1", nap_explain, 6, "spawn")
    + "revoke ai1 spawn\n"
    + "revoked ai1 spawn\n"
    + "gate: request 3 cancelled: ai1 may no longer spawn\n"
    + "R3GATE reply agent=ai1 seq=6 verb=spawn result=denied rq=3 reason=revoked\n"
    + p
    + "replay\n"
    + entries(range(1, 21))
    + "replay ok entries=20 lost=0 head=H\n"
    + p
    + "replay 11\n"
    + entry(11, log_records[10][1], log_records[10][2])
    + "replay step 11 of 21 ok\n"
    + p
    + "log\n"
    + "log entries=22 of 128 lost=0 head=H\n"
    + entries(range(18, 23))
    + p
    + "exit\n"
    + exit_ok,
)

full_steps = ["line grant ai1 spawn\n", "ai R3GATE ASK ai1 1 spawn nap\n"]
for k in range(2, 119):
    full_steps.append("ai R3GATE ASK zed %d read\n" % k)
full_steps.append("ai R3GATE ASK zed 119 read\n")
for _ in range(7):
    full_steps.append("line echo a\n")
full_steps.extend([
    "line echo b\n",
    "line approve 1\n",
    "line ps\n",
    "line deny 1\n",
    "line replay\n",
    "line log\n",
    "line exit\n",
])
write_text("log-full.steps", "".join(full_steps))
full_parts = [
    prefix,
    typed("grant ai1 spawn", "granted ai1 spawn"),
    pending_block(1, "ai1", nap_explain, 1, "spawn"),
]
for k in range(2, 119):
    full_parts.append(refused("zed", "no badge", k, "read", "no-badge"))
full_parts.append(
    "\n"
    "gate: ignored an agent line: flight recorder full\n"
    "R3GATE reply agent=zed seq=119 verb=read result=denied reason=log-full\n"
    + p
)
for _ in range(7):
    full_parts.append(typed("echo a", "a"))
full_parts.append("echo b\nb\nlog: flight recorder full; later entries are counted as lost\n" + p)
full_parts.append(typed("approve 1", "error: flight recorder full: approve refused"))
full_parts.append(typed("ps", ps_two[:-1]))
full_parts.append(
    "deny 1\n"
    "gate: request 1 denied\n"
    "R3GATE reply agent=ai1 seq=1 verb=spawn result=denied rq=1 reason=human\n"
    + p
)
full_parts.append("replay\n")
full_parts.append(entry(1, "gate", "grant ai1 spawn"))
full_parts.append(entry(2, "human last=ok", "grant ai1 spawn"))
full_parts.append(entry(3, "agent ai1", "spawn-nap seq=1 rq=1 pending"))
for n in range(4, 121):
    full_parts.append(entry(n, "agent zed", "read seq=%d denied reason=no-badge" % (n - 2)))
for n in range(121, 128):
    full_parts.append(entry(n, "human last=ok", "echo a"))
full_parts.append(entry(128, "human last=ok", "echo b"))
full_parts.append("replay ok entries=128 lost=5 head=H\n" + p)
full_parts.append("log\nlog entries=128 of 128 lost=6 head=H\n")
for n in range(124, 128):
    full_parts.append(entry(n, "human last=ok", "echo a"))
full_parts.append(entry(128, "human last=ok", "echo b"))
full_parts.append(p + "exit\n" + exit_ok)
write_bytes("log-full.expected", "".join(full_parts))

usage_log = [
    ("log x", "error: usage: log"),
    ("replay 1 2", "error: usage: replay [N]"),
    ("replay x", "error: no log entry x"),
    ("replay 0", "error: no log entry 0"),
    ("replay 9", "error: no log entry 9"),
    ("selftest x", "error: usage: selftest"),
]
write_text("log-usage.steps", "".join("line " + text + "\n" for text, _ in usage_log) + "line exit\n")
write_bytes(
    "log-usage.expected",
    prefix + "".join(typed(text, output) for text, output in usage_log) + "exit\n" + exit_cmd,
)

card_names = ["timer", "sched", "frames", "script", "gate", "recorder", "store", "heap"]
card = "selftest: 8 checks\n" + "".join("selftest %s ok\n" % name.ljust(8) for name in card_names) + "selftest: 8 of 8 ok"
write_text(
    "shell-selftest.steps",
    "line grant ai1 read\nline selftest\nline badges\nline ps\nline exit\n",
)
write_bytes(
    "shell-selftest.expected",
    prefix
    + typed("grant ai1 read", "granted ai1 read")
    + "selftest\n"
    + card
    + "\n"
    + p
    + typed("badges", "badge ai1 may=read\nno pending request")
    + typed("ps", ps_two[:-1])
    + "exit\n"
    + exit_ok,
)
store_records = [
    (1, "human last=ok", "store"),
    (2, "human last=ok", "save note hello there"),
    (3, "human last=ok", "show"),
    (4, "human last=ok", "show note"),
    (5, "gate", "grant ai1 say"),
    (6, "human last=ok", "grant ai1 say"),
    (7, "agent ai1", "say seq=1 done text=hi"),
    (8, "agent ai1", "- seq=2 bad reason=verb"),
    (9, "human last=ok", "forget note"),
    (10, "human last=ok", "show"),
    (11, "human last=ok", "boots"),
    (12, "human last=ok", "boots 1"),
]

def store_entries(nums):
    return "".join(
        "#%d t=T %s link=L\n  %s\n" % (n, store_records[n - 1][1], store_records[n - 1][2])
        for n in nums
    )

write_text(
    "store-ram.steps",
    "line store\n"
    "line save note hello there\n"
    "line show\n"
    "line show note\n"
    "line grant ai1 say\n"
    "ai R3GATE ASK ai1 1 say hi\n"
    "ai R3GATE ASK ai1 2 save x y\n"
    "line forget note\n"
    "line show\n"
    "line boots\n"
    "line boots 1\n"
    "line replay\n"
    "line exit\n",
)
write_bytes(
    "store-ram.expected",
    prefix
    + typed(
        "store",
        "store device=ram blocks=512 boots=1 of 15 saved=0 of 16\n"
        "store: ram disk, erased by a reboot; this boot is boot 1",
    )
    + typed("save note hello there", "saved note to ram")
    + typed("show", "saved note\nsaved 1 of 16 on ram")
    + typed("show note", "note = hello there")
    + typed("grant ai1 say", "granted ai1 say")
    + "\n"
    + "ai1 says: hi\n"
    + "R3GATE reply agent=ai1 seq=1 verb=say result=done\n"
    + p
    + bad_line("verb", "ai1", "2", "-")
    + typed("forget note", "forgot note")
    + typed("show", "saved 0 of 16 on ram")
    + "boots\n"
    + "boot 1 entries=10 lost=0 head=H chain=ok prev=-\n"
    + "boots 1 of 15 used; this is boot 1\n"
    + p
    + "boots 1\n"
    + store_entries(range(1, 12))
    + "boot 1 ok entries=11 lost=0 head=H\n"
    + p
    + "replay\n"
    + store_entries(range(1, 13))
    + "replay ok entries=12 lost=0 head=H\n"
    + p
    + "exit\n"
    + exit_ok,
)

long_text = "x" * 69
if len("save a " + long_text) != 76:
    raise SystemExit("store-usage line length")
usage_store = [
    ("store x", "error: usage: store"),
    ("boots 1 2", "error: usage: boots [K]"),
    ("boots x", "error: no boot x"),
    ("boots 0", "error: no boot 0"),
    ("boots 2", "error: no boot 2"),
    ("save", "error: usage: save NAME TEXT"),
    ("save a", "error: usage: save NAME TEXT"),
    ("save A hi", "error: bad saved name: A"),
    ("save abcdefghijklm hi", "error: bad saved name: abcdefghijklm"),
    ("save a " + long_text, "error: saved text too long (max 58)"),
    ("show x y", "error: usage: show [NAME]"),
    ("show zz", "error: no saved name zz"),
    ("forget", "error: usage: forget NAME"),
    ("forget zz", "error: no saved name zz"),
]
for n in range(1, 17):
    usage_store.append(("save k%d v" % n, "saved k%d to ram" % n))
usage_store.append(("save k1 w", "saved k1 to ram"))
usage_store.append(("save k17 v", "error: saved data full (16 names)"))
write_text("store-usage.steps", "".join("line " + cmd + "\n" for cmd, _ in usage_store) + "line exit\n")
write_bytes(
    "store-usage.expected",
    prefix + "".join(typed(cmd, output) for cmd, output in usage_store) + "exit\n" + exit_cmd,
)

reboot_boot1 = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_SHELL_READY\n"
    "R3@TH3 shell. Type help for commands.\n"
    "store: formatted a blank nvme disk\n"
    "store: nvme disk; this boot is saved as boot 1 of 64\n"
    "r3> save note kept\n"
    "saved note to nvme\n"
    "r3> grant ai1 say\n"
    "granted ai1 say\n"
    "r3> \n"
    "ai1 says: hi\n"
    "R3GATE reply agent=ai1 seq=1 verb=say result=done\n"
    "r3> log\n"
    "log entries=4 of 128 lost=0 head=H\n"
    "#1 t=T human last=ok link=L\n"
    "  save note kept\n"
    "#2 t=T gate link=L\n"
    "  grant ai1 say\n"
    "#3 t=T human last=ok link=L\n"
    "  grant ai1 say\n"
    "#4 t=T agent ai1 link=L\n"
    "  say seq=1 done text=hi\n"
    "r3> exit\n"
    "R3ATH3_SHELL_EXIT last=ok\n"
)
reboot_boot2 = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_SHELL_READY\n"
    "R3@TH3 shell. Type help for commands.\n"
    "store: nvme disk; this boot is saved as boot 2 of 64\n"
    "r3> show note\n"
    "note = kept\n"
    "r3> boots\n"
    "boot 1 entries=5 lost=0 head=H chain=ok prev=-\n"
    "boot 2 entries=1 lost=0 head=H chain=ok prev=ok\n"
    "boots 2 of 64 used; this is boot 2\n"
    "r3> boots 1\n"
    "#1 t=T human last=ok link=L\n"
    "  save note kept\n"
    "#2 t=T gate link=L\n"
    "  grant ai1 say\n"
    "#3 t=T human last=ok link=L\n"
    "  grant ai1 say\n"
    "#4 t=T agent ai1 link=L\n"
    "  say seq=1 done text=hi\n"
    "#5 t=T human last=ok link=L\n"
    "  log\n"
    "boot 1 ok entries=5 lost=0 head=H\n"
    "r3> exit\n"
    "R3ATH3_SHELL_EXIT last=ok\n"
)
tamper_boot2 = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_SHELL_READY\n"
    "R3@TH3 shell. Type help for commands.\n"
    "store: nvme disk; this boot is saved as boot 2 of 64\n"
    "r3> boots\n"
    "boot 1 entries=2 lost=0 head=H chain=#3 prev=-\n"
    "boot 2 entries=0 lost=0 head=H chain=ok prev=ok\n"
    "boots 2 of 64 used; this is boot 2\n"
    "r3> boots 1\n"
    "#1 t=T human last=ok link=L\n"
    "  save note kept\n"
    "#2 t=T gate link=L\n"
    "  grant ai1 say\n"
    "boot 1: chain broken at #3\n"
    "r3> exit\n"
    "R3ATH3_SHELL_EXIT last=cmd\n"
)
baddisk = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_SHELL_READY\n"
    "R3@TH3 shell. Type help for commands.\n"
    "store: nvme disk not used (superblock); using the ram disk, erased by a reboot\n"
    "r3> store\n"
    "store device=ram blocks=512 boots=1 of 15 saved=0 of 16\n"
    "store: ram disk, erased by a reboot; this boot is boot 1\n"
    "store: nvme disk not used (superblock)\n"
    "r3> exit\n"
    "R3ATH3_SHELL_EXIT last=ok\n"
)
write_text(
    "store-reboot.boot1.steps",
    "line save note kept\nline grant ai1 say\nai R3GATE ASK ai1 1 say hi\nline log\nline exit\n",
)
write_text(
    "store-reboot.boot2.steps",
    "line show note\nline boots\nline boots 1\nline exit\n",
)
write_text(
    "store-tamper.boot2.steps",
    "line boots\nline boots 1\nline exit\n",
)
write_text("store-baddisk.steps", "line store\nline exit\n")
write_bytes("store-reboot.boot1.expected", reboot_boot1)
write_bytes("store-reboot.boot2.expected", reboot_boot2)
write_bytes("store-tamper.boot2.expected", tamper_boot2)
write_bytes("store-baddisk.expected", baddisk)

wasm_core = (
    "R3ATH3_STAGE rust_main\n"
    "R3ATH3_WASM_CORE start\n"
    "R3ATH3_WASM_CORE step=1 add=42\n"
    "R3ATH3_WASM_CORE step=2 fac64=2432902008176640000 ext8=-1 bend=-1\n"
    "R3ATH3_WASM_CORE step=3 fib=6765 pick=10,20,99 ind=7,8\n"
    "R3ATH3_WASM_CORE step=4 mem=305419896 grow=1,-1 size=2\n"
    "R3ATH3_WASM_CORE step=5 traps=unreachable,memory,div-zero,overflow,indirect,stack,fuel brtable=fuel\n"
    "R3ATH3_WASM_CORE step=6 after-trap=dead fresh=42\n"
    "R3ATH3_WASM_CORE step=7 fresh-memory=0\n"
    "R3ATH3_WASM_CORE step=8 refused=magic,version,truncated,section,opcode,float,import,limits\n"
    "R3ATH3_WASM_CORE step=9 prefixes=ok flips=ok\n"
    "R3ATH3_WASM_CORE step=10 hello=abi-ok spin-init=fuel\n"
    "R3ATH3_WASM_CORE step=11 heap=same frames=same\n"
    "R3ATH3_WASM_CORE ok steps=11\n"
)
write_bytes("wasm-core.expected", wasm_core)

write_text("app-hello.steps", "launch hello\nxp\nappquit\nline exit\n")
write_bytes(
    "app-hello.expected",
    prefix
    + "app hello\n"
    + "app hello: started; q quits\n"
    + "R3ATH3_APP_READY name=hello\n"
    + "app hello: quit\n"
    + p
    + "exit\n"
    + exit_ok,
)
hello_rows = []
for row in range(25):
    cells = []
    for col in range(80):
        glyph = 0x20
        index = row * 80 + col
        if 990 <= index < 990 + len("hello from wasm"):
            glyph = ord("hello from wasm"[index - 990])
        elif 1150 <= index < 1150 + len("press q to quit"):
            glyph = ord("press q to quit"[index - 1150])
        cells.append("%04X" % (0x0A00 | glyph))
    hello_rows.append(" ".join(cells))
write_text("app-hello.vga", "\n".join(hello_rows) + "\n")

app_list = (
    "app glue-duty  Glue Duty, the belt-scrub game, in text mode\n"
    "app hello      a static screen; the smallest app\n"
    "app spin       loops forever; the fuel limit stops it"
)
write_text("app-spin.steps", "line app\nline app spin\nline echo alive\nline exit\n")
write_bytes(
    "app-spin.expected",
    prefix
    + typed("app", app_list)
    + typed("app spin", "app spin: started; q quits\napp spin: stopped: trap fuel")
    + typed("echo alive", "alive")
    + "exit\n"
    + exit_ok,
)
write_text("app-usage.steps", "line app a b\nline app nope\nline app Spin\nline exit\n")
write_bytes(
    "app-usage.expected",
    prefix
    + typed("app a b", "error: usage: app [NAME]")
    + typed("app nope", "error: no app nope")
    + typed("app Spin", "error: no app Spin")
    + "exit\n"
    + exit_cmd,
)
write_text(
    "app-glue.steps",
    "launch glue-duty\nxp\nappkey spc 1\nappkey p 1\nappkey p 1\nappquit\nline exit\n",
)
write_bytes(
    "app-glue.expected",
    prefix
    + "app glue-duty\n"
    + "app glue-duty: started; q quits\n"
    + "R3ATH3_APP_READY name=glue-duty\n"
    + "R3ATH3_APP_STATE name=glue-duty state=1\n"
    + "R3ATH3_APP_STATE name=glue-duty state=2\n"
    + "R3ATH3_APP_STATE name=glue-duty state=1\n"
    + "app glue-duty: quit; shift 1 score 0 cleaned 0 gum-up 0%\n"
    + p
    + "exit\n"
    + exit_ok,
)
write_text(
    "app-gate.steps",
    "line grant ai1 app\n"
    "ai R3GATE ASK ai1 1 app spin\n"
    "line grant ai1 say\n"
    "ai R3GATE ASK ai1 2 say app spin\n"
    "line app spin\n"
    "line badges\n"
    "line exit\n",
)
write_bytes(
    "app-gate.expected",
    prefix
    + typed("grant ai1 app", "error: unknown permission: app")
    + bad_line("verb", "ai1", "1", "-")
    + typed("grant ai1 say", "granted ai1 say")
    + "\n"
    + "ai1 says: app spin\n"
    + "R3GATE reply agent=ai1 seq=2 verb=say result=done\n"
    + p
    + typed("app spin", "app spin: started; q quits\napp spin: stopped: trap fuel")
    + typed("badges", "badge ai1 may=say\nno pending request")
    + "exit\n"
    + exit_ok,
)
PY
}

expect_shell() {
  local name="$1"
  local seconds="$2"
  local steps="$3"
  local expected="$4"
  local want="$5"
  local kind="$6"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  local sock="$OUT/$name.mon.sock"
  local vga_file="$OUT/$name.vga"
  local elf="$OUT/shell.elf"
  rm -f "$sock" "$vga_file" "$log_file"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - \
    "$elf" "$log_file" "$err_file" "$seconds" "$steps" "$sock" "$vga_file" "$OUT/harness_py.py" <<'PY'
import importlib.util, os, re, signal, socket, subprocess, sys, time
elf, out_log, err_log, seconds, steps_path, sock_path, vga_path, harness_path = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", harness_path)
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
# A previous run's transcript must not satisfy the ready wait. QEMU truncates
# the serial file only after it starts, and the first read can happen before that.
try:
    os.remove(out_log)
except FileNotFoundError:
    pass
err_f = open(err_log, "wb")
proc = subprocess.Popen(
    [
        "qemu-system-x86_64", "-machine", "pc,accel=tcg", "-cpu", "qemu64",
        "-m", "64M", "-smp", "1", "-kernel", elf, "-nic", "none",
        "-display", "none", "-vga", "std", "-serial", "file:" + out_log,
        "-monitor", "unix:" + sock_path + ",server,nowait", "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ],
    stdout=subprocess.DEVNULL, stderr=err_f,
)

def stop(_signum, _frame):
    if proc.poll() is None:
        proc.kill()
    sys.exit(124)

signal.signal(signal.SIGTERM, stop)
deadline = time.time() + float(seconds)

def fail(code, message):
    sys.stderr.write(message + "\n")
    try:
        err_f.write((message + "\n").encode())
        err_f.flush()
    except OSError:
        pass
    if proc.poll() is None:
        proc.kill()
        proc.wait()
    sys.exit(code)

def read_serial():
    try:
        return open(out_log, "rb").read()
    except FileNotFoundError:
        return b""

READY_ECHO = b"R3ATH3_SHELL_READY\nR3@TH3 shell. Type help for commands."

def step_settled(data, start, echo, keys_mode):
    if keys_mode:
        return harness.keys_at_prompt(data, start)
    return harness.idle_at_prompt(data, start, echo)

def wait_stable(start, echo, keys_mode):
    limit = time.time() + 5
    while time.time() < limit and time.time() < deadline:
        data = read_serial()
        if step_settled(data, start, echo, keys_mode):
            time.sleep(0.05)
            data2 = read_serial()
            if len(data2) == len(data) and step_settled(data2, start, echo, keys_mode):
                return True
            continue
        if proc.poll() is not None:
            return False
        time.sleep(0.02)
    return False

PLAIN = {
    " ": "spc",
    "-": "minus",
    "=": "equal",
    ";": "semicolon",
    "'": "apostrophe",
}
SHIFTED = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
    "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
    "_": "minus", "+": "equal", ":": "semicolon", '"': "apostrophe",
}

def qcodes_for_text(text):
    keys = []
    for ch in text:
        if ("a" <= ch <= "z") or ("0" <= ch <= "9"):
            keys.append(ch)
        elif "A" <= ch <= "Z":
            keys.append("shift-" + ch.lower())
        elif ch in PLAIN:
            keys.append(PLAIN[ch])
        elif ch in SHIFTED:
            keys.append("shift-" + SHIFTED[ch])
        else:
            fail(2, "no qcode for " + repr(ch))
    keys.append("ret")
    return keys

steps = []
for raw in open(steps_path, "r", encoding="utf-8"):
    raw = raw.rstrip("\n")
    if raw == "" or raw.startswith("#"):
        continue
    if raw == "line":
        steps.append(("line", qcodes_for_text(""), b""))
    elif raw.startswith("line "):
        steps.append(("line", qcodes_for_text(raw[5:]), raw[5:].encode("utf-8")))
    elif raw.startswith("keys "):
        steps.append(("keys", raw[5:].split(), None))
    elif raw == "xp":
        steps.append(("xp", [], None))
    elif raw.startswith("launch "):
        steps.append(("launch", qcodes_for_text("app " + raw[7:]), raw[7:].encode("utf-8")))
    elif raw == "appquit":
        steps.append(("appquit", ["q"], None))
    elif raw.startswith("appkey "):
        parts = raw.split()
        if len(parts) != 3:
            fail(2, "bad appkey " + raw)
        steps.append(("appkey", [parts[1], parts[2]], None))
    else:
        fail(2, "bad step " + raw)

ready_deadline = min(deadline, time.time() + 5)
ready = False
while time.time() < ready_deadline:
    if proc.poll() is not None:
        break
    data = read_serial()
    if harness.idle_at_prompt(data, 0, READY_ECHO):
        time.sleep(0.05)
        data2 = read_serial()
        if len(data2) == len(data) and harness.idle_at_prompt(data2, 0, READY_ECHO):
            ready = True
            break
        continue
    time.sleep(0.02)
if not ready:
    fail(124, "timed out waiting for the shell prompt")

mon = None
for _ in range(50):
    if os.path.exists(sock_path):
        candidate = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            candidate.connect(sock_path)
            mon = candidate
            break
        except OSError:
            candidate.close()
    time.sleep(0.05)
if mon is None:
    fail(1, "monitor socket did not appear: " + sock_path)
mon.settimeout(0.2)
try:
    mon.recv(4096)
except Exception:
    pass

ANSI = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]")

def parse_cells(buf):
    clean = ANSI.sub(b"", buf)
    cells = []
    for line in clean.splitlines():
        line = line.strip()
        match = re.match(rb"^[0-9a-fA-F]+:((?: 0x[0-9a-fA-F]{4})+)$", line)
        if not match:
            continue
        for hx in re.findall(rb"0x([0-9a-fA-F]{4})", match.group(1)):
            cells.append(int(hx, 16))
    return cells

def send_keys(keys):
    for key in keys:
        try:
            mon.sendall(("sendkey " + key + "\n").encode())
        except OSError:
            return False
        time.sleep(0.03)
    return True

for index, (kind, keys, echo) in enumerate(steps):
    last = index + 1 == len(steps)
    if kind in ("line", "keys"):
        # Snapshot before the first key. The shell echoes while keys are
        # still being sent, so the echo has to land after this point.
        start = len(read_serial())
        if not send_keys(keys):
            fail(1, "monitor closed while sending keys")
        if not last and not wait_stable(start, echo, kind == "keys"):
            fail(124, "timed out waiting for the next prompt")
    elif kind == "launch":
        start = len(read_serial())
        if not send_keys(keys):
            fail(1, "monitor closed while sending keys")
        ready_line = b"R3ATH3_APP_READY name=" + echo + b"\n"
        limit = time.time() + 5
        ready = False
        while time.time() < limit and time.time() < deadline:
            data = read_serial()
            if ready_line in data[start:]:
                time.sleep(0.05)
                if ready_line in read_serial()[start:]:
                    ready = True
                    break
            if proc.poll() is not None:
                break
            time.sleep(0.02)
        if not ready:
            fail(124, "timed out waiting for the app ready line")
    elif kind == "appquit":
        start = len(read_serial())
        if not send_keys(keys):
            fail(1, "monitor closed while sending keys")
        limit = time.time() + 5
        done = False
        while time.time() < limit and time.time() < deadline:
            data = read_serial()
            chunk = data[start:]
            if (b": quit" in chunk or b": stopped:" in chunk) and harness.prompt_anchored(data):
                time.sleep(0.05)
                data2 = read_serial()
                chunk2 = data2[start:]
                if len(data2) == len(data) and (b": quit" in chunk2 or b": stopped:" in chunk2) and harness.prompt_anchored(data2):
                    done = True
                    break
            if proc.poll() is not None:
                break
            time.sleep(0.02)
        if not done:
            fail(124, "timed out waiting for the app to end")
    elif kind == "appkey":
        key, count = keys
        start = len(read_serial())
        if not send_keys([key]):
            fail(1, "monitor closed while sending keys")
        need = int(count)
        if need == 0:
            time.sleep(3)
        else:
            limit = time.time() + 5
            got = False
            while time.time() < limit and time.time() < deadline:
                if read_serial()[start:].count(b"R3ATH3_APP_STATE ") >= need:
                    got = True
                    break
                if proc.poll() is not None:
                    break
                time.sleep(0.02)
            if not got:
                fail(124, "timed out waiting for app state")
    elif kind == "xp":
        try:
            mon.sendall(b"xp /2000hx 0xb8000\n")
        except OSError:
            fail(1, "monitor closed before xp")
        buf = b""
        cells = []
        xp_deadline = time.time() + 2
        while time.time() < xp_deadline and len(cells) < 2000:
            try:
                chunk = mon.recv(65536)
            except socket.timeout:
                chunk = b""
            if chunk:
                buf += chunk
                cells = parse_cells(buf)
            else:
                time.sleep(0.01)
        if len(cells) < 2000:
            fail(1, "xp returned %d cells" % len(cells))
        lines = []
        for row in range(25):
            chunk = cells[row * 80:(row + 1) * 80]
            lines.append(" ".join("%04X" % cell for cell in chunk))
        open(vga_path, "w", encoding="ascii").write("\n".join(lines) + "\n")
    else:
        fail(2, "unknown step " + kind)

while proc.poll() is None:
    if time.time() > deadline:
        fail(124, "timed out waiting for QEMU to exit")
    time.sleep(0.02)
code = proc.wait()
try:
    mon.close()
except OSError:
    pass
sys.exit(code if code is not None else 124)
PY
  local status=$?
  set -e
  local passed=0
  case "$kind" in
    exact)
      if judge_exact "$status" "$log_file" "$want" "$expected"; then
        passed=1
      fi
      ;;
    mem)
      if judge_shell_mem "$status" "$log_file" "$expected" && shell_frames_in_bound "$log_file"; then
        passed=1
      fi
      ;;
    screen)
      if judge_shell_screen "$status" "$log_file" "$expected" "$vga_file" "$OUT/task/shell-screen.vga"; then
        passed=1
      fi
      ;;
    apphello)
      if judge_app_screen "$status" "$log_file" "$expected" "$vga_file" "$OUT/task/app-hello.vga"; then
        passed=1
      fi
      ;;
    appglue)
      if judge_app_screen "$status" "$log_file" "$expected" "$vga_file" "$ROOT/apps/glue-duty/golden/app-glue.vga"; then
        passed=1
      fi
      ;;
    *)
      log "[FAIL] $name unknown judge $kind"
      FAILURES=$((FAILURES + 1))
      return
      ;;
  esac
  if [[ "$passed" -eq 1 ]]; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$log_file" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected" 2>/dev/null || true
    if [[ "$kind" == "screen" || "$kind" == "apphello" || "$kind" == "appglue" ]]; then
      log "----- vga -----"
      cat "$vga_file" 2>/dev/null || true
      log "----- vga expected -----"
      if [[ "$kind" == "apphello" ]]; then
        cat "$OUT/task/app-hello.vga" 2>/dev/null || true
      elif [[ "$kind" == "appglue" ]]; then
        cat "$ROOT/apps/glue-duty/golden/app-glue.vga" 2>/dev/null || true
      else
        cat "$OUT/task/shell-screen.vga" 2>/dev/null || true
      fi
    fi
    log "----- qemu stderr -----"
    cat "$err_file" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

# Runs one gate transcript. Prints the QEMU status. An optional disk path
# attaches the emulated NVMe controller. Judging stays in expect_gate.
gate_qemu() {
  local name="$1"
  local seconds="$2"
  local steps="$3"
  local disk="${4:-}"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  local sock="$OUT/$name.mon.sock"
  local ser="$OUT/$name.ser.sock"
  local elf="$OUT/shell.elf"
  rm -f "$sock" "$ser" "$log_file"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - \
    "$elf" "$log_file" "$err_file" "$seconds" "$steps" "$sock" "$ser" "$OUT/harness_py.py" "$disk" <<'PY'
import importlib.util, os, signal, socket, subprocess, sys, time
args = sys.argv[1:]
elf, out_log, err_log, seconds, steps_path, sock_path, ser_path, harness_path = args[:8]
disk = args[8] if len(args) > 8 else ""
spec = importlib.util.spec_from_file_location("harness_py", harness_path)
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
for path in (out_log, ser_path, sock_path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass
err_f = open(err_log, "wb")
cmd = [
    "qemu-system-x86_64", "-machine", "pc,accel=tcg", "-cpu", "qemu64",
    "-m", "64M", "-smp", "1", "-kernel", elf, "-nic", "none",
    "-display", "none", "-vga", "std",
    "-chardev", "socket,id=com1,path=" + ser_path + ",server=on,wait=on",
    "-serial", "chardev:com1",
    "-monitor", "unix:" + sock_path + ",server,nowait",
    "-no-reboot",
]
if disk:
    cmd += [
        "-drive", "file=%s,if=none,id=r3disk,format=raw" % disk,
        "-device", "nvme,serial=r3ath3,drive=r3disk",
    ]
cmd += ["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]
proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=err_f)
buf = b""
ser = None
deadline = time.time() + float(seconds)

def read_serial():
    global buf
    if ser is None:
        return buf
    while True:
        try:
            chunk = ser.recv(4096)
        except BlockingIOError:
            break
        except InterruptedError:
            continue
        if not chunk:
            break
        buf += chunk
    return buf

def flush_log():
    try:
        read_serial()
    except Exception:
        pass
    try:
        open(out_log, "wb").write(buf)
    except OSError:
        pass

def stop(_signum, _frame):
    flush_log()
    if proc.poll() is None:
        proc.kill()
    sys.exit(124)

signal.signal(signal.SIGTERM, stop)

def fail(code, message):
    sys.stderr.write(message + "\n")
    try:
        err_f.write((message + "\n").encode())
        err_f.flush()
    except OSError:
        pass
    if proc.poll() is None:
        proc.kill()
        proc.wait()
    flush_log()
    sys.exit(code)

def connect(path, tries, delay):
    for _ in range(tries):
        if os.path.exists(path):
            candidate = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                candidate.connect(path)
                return candidate
            except OSError:
                candidate.close()
        if proc.poll() is not None:
            break
        time.sleep(delay)
    return None

ser_conn = connect(ser_path, 50, 0.05)
if ser_conn is None:
    fail(1, "serial socket did not connect: " + ser_path)
ser = ser_conn
ser.setblocking(False)
mon = connect(sock_path, 50, 0.05)
if mon is None:
    fail(1, "monitor socket did not appear: " + sock_path)
mon.settimeout(0.2)
try:
    mon.recv(4096)
except Exception:
    pass

READY_ECHO = b"R3ATH3_SHELL_READY\nR3@TH3 shell. Type help for commands."

def step_settled(data, start, echo, keys_mode):
    if keys_mode:
        return harness.keys_at_prompt(data, start)
    return harness.idle_at_prompt(data, start, echo)

def wait_stable(start, echo, keys_mode):
    limit = time.time() + 5
    while time.time() < limit and time.time() < deadline:
        data = read_serial()
        if step_settled(data, start, echo, keys_mode):
            time.sleep(0.05)
            data2 = read_serial()
            if len(data2) == len(data) and step_settled(data2, start, echo, keys_mode):
                return True
            continue
        if proc.poll() is not None:
            return False
        time.sleep(0.02)
    return False

def wait_ai(start):
    limit = time.time() + 5
    while time.time() < limit and time.time() < deadline:
        data = read_serial()
        if b"R3GATE reply " in data[start:] and harness.prompt_anchored(data):
            time.sleep(0.05)
            data2 = read_serial()
            if len(data2) == len(data) and b"R3GATE reply " in data2[start:] and harness.prompt_anchored(data2):
                return True
            continue
        if proc.poll() is not None:
            return False
        time.sleep(0.02)
    return False

PLAIN = {
    " ": "spc",
    "-": "minus",
    "=": "equal",
    ";": "semicolon",
    "'": "apostrophe",
}
SHIFTED = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
    "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
    "_": "minus", "+": "equal", ":": "semicolon", '"': "apostrophe",
}

def qcodes_for_text(text):
    keys = []
    for ch in text:
        if ("a" <= ch <= "z") or ("0" <= ch <= "9"):
            keys.append(ch)
        elif "A" <= ch <= "Z":
            keys.append("shift-" + ch.lower())
        elif ch in PLAIN:
            keys.append(PLAIN[ch])
        elif ch in SHIFTED:
            keys.append("shift-" + SHIFTED[ch])
        else:
            fail(2, "no qcode for " + repr(ch))
    keys.append("ret")
    return keys

steps = []
for raw in open(steps_path, "r", encoding="utf-8"):
    raw = raw.rstrip("\n")
    if raw == "" or raw.startswith("#"):
        continue
    if raw == "line":
        steps.append(("line", qcodes_for_text(""), b""))
    elif raw.startswith("line "):
        steps.append(("line", qcodes_for_text(raw[5:]), raw[5:].encode("utf-8")))
    elif raw.startswith("keys "):
        steps.append(("keys", raw[5:].split(), None))
    elif raw.startswith("ai "):
        steps.append(("ai", raw[3:], None))
    else:
        fail(2, "bad step " + raw)

ready_deadline = min(deadline, time.time() + 5)
ready = False
while time.time() < ready_deadline:
    if proc.poll() is not None:
        break
    data = read_serial()
    if harness.idle_at_prompt(data, 0, READY_ECHO):
        time.sleep(0.05)
        data2 = read_serial()
        if len(data2) == len(data) and harness.idle_at_prompt(data2, 0, READY_ECHO):
            ready = True
            break
        continue
    time.sleep(0.02)
if not ready:
    fail(124, "timed out waiting for the shell prompt")

def send_keys(keys):
    for key in keys:
        try:
            mon.sendall(("sendkey " + key + "\n").encode())
        except OSError:
            return False
        time.sleep(0.03)
    return True

for index, (kind, payload, echo) in enumerate(steps):
    last = index + 1 == len(steps)
    if kind in ("line", "keys"):
        start = len(read_serial())
        if not send_keys(payload):
            fail(1, "monitor closed while sending keys")
        if not last and not wait_stable(start, echo, kind == "keys"):
            fail(124, "timed out waiting for the next prompt")
    elif kind == "ai":
        start = len(read_serial())
        view = memoryview((payload + "\n").encode("utf-8"))
        try:
            for offset in range(0, len(view), 8):
                ser.sendall(view[offset:offset + 8])
                time.sleep(0.001)
        except OSError:
            fail(1, "serial closed while sending an agent line")
        if not wait_ai(start):
            fail(124, "timed out waiting for the gate reply")
    else:
        fail(2, "unknown step " + kind)

while proc.poll() is None:
    read_serial()
    if time.time() > deadline:
        fail(124, "timed out waiting for QEMU to exit")
    time.sleep(0.02)
code = proc.wait()
try:
    mon.close()
except OSError:
    pass
flush_log()
sys.exit(code if code is not None else 124)
PY
  local status=$?
  set -e
  echo "$status"
}

expect_gate() {
  local name="$1"
  local seconds="$2"
  local steps="$3"
  local expected="$4"
  local want="$5"
  local kind="${6:-exact}"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  local status
  status="$(gate_qemu "$name" "$seconds" "$steps")"
  local passed=0
  case "$kind" in
    exact)
      if judge_exact "$status" "$log_file" "$want" "$expected"; then
        passed=1
      fi
      ;;
    log)
      if judge_shell_log "$status" "$log_file" "$expected" "$want"; then
        passed=1
      fi
      ;;
    store)
      if judge_shell_store "$status" "$log_file" "$expected" "$want" "" ""; then
        passed=1
      fi
      ;;
    store:s1)
      if judge_shell_store "$status" "$log_file" "$expected" "$want" "s1=live" ""; then
        passed=1
      fi
      ;;
    *)
      log "[FAIL] $name unknown judge $kind"
      FAILURES=$((FAILURES + 1))
      return
      ;;
  esac
  if [[ "$passed" -eq 1 ]]; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$log_file" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$err_file" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

make_image() {
  local path="$1"
  local kind="$2"
  python3 - "$path" "$kind" <<'PY'
import sys
path, kind = sys.argv[1:]
data = bytearray(2097152)
if kind == "badsuper":
    data[:8] = b"NOTR3ATH"
elif kind != "blank":
    raise SystemExit("bad image kind")
open(path, "wb").write(data)
PY
}

apply_mutation() {
  local img="$1"
  local mutation="$2"
  if [[ "$mutation" == "none" ]]; then
    return 0
  fi
  case "$mutation" in
    flip:*)
      python3 - "$img" "${mutation#flip:}" <<'PY'
import sys
path, offset = sys.argv[1], int(sys.argv[2])
data = bytearray(open(path, "rb").read())
data[offset] ^= 0x01
open(path, "wb").write(data)
PY
      ;;
    *)
      return 1
      ;;
  esac
}

expect_nvme_core() {
  local name="nvme-core"
  local img="$OUT/$name.img"
  rm -f "$img" "$OUT/$name.serial.log" "$OUT/$name.qemu.err"
  make_image "$img" blank
  local status
  status="$(run_qemu "$name" 12 "$img")"
  local image_ok=1
  if ! python3 - "$OUT/harness_py.py" "$img" <<'PY'
import importlib.util, sys
path, image = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
sys.exit(0 if mod.nvme_image_ok(open(image, "rb").read()) else 1)
PY
  then
    image_ok=0
  fi
  if judge_nvme_core "$status" "$OUT/$name.serial.log" && [[ "$image_ok" -eq 1 ]]; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

expect_reboot() {
  local name="$1"
  local steps1="$2"
  local expected1="$3"
  local want1="$4"
  local mutation="$5"
  local steps2="$6"
  local expected2="$7"
  local want2="$8"
  local img="$OUT/$name.img"
  rm -f "$img" \
    "$OUT/$name.boot1.serial.log" "$OUT/$name.boot1.qemu.err" \
    "$OUT/$name.boot2.serial.log" "$OUT/$name.boot2.qemu.err"
  make_image "$img" blank
  local status
  status="$(gate_qemu "$name.boot1" 20 "$steps1" "$img")"
  if ! judge_shell_store "$status" "$OUT/$name.boot1.serial.log" "$expected1" "$want1" "" ""; then
    log "[FAIL] $name exit=$status"
    log "----- boot 1 -----"
    cat "$OUT/$name.boot1.serial.log" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected1" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.boot1.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
    return
  fi
  if ! apply_mutation "$img" "$mutation"; then
    log "[FAIL] $name mutation"
    FAILURES=$((FAILURES + 1))
    return
  fi
  local status2
  status2="$(gate_qemu "$name.boot2" 20 "$steps2" "$img")"
  if judge_shell_store "$status2" "$OUT/$name.boot2.serial.log" "$expected2" "$want2" "" "$OUT/$name.boot1.serial.log"; then
    log "[PASS] $name exit=$status,$status2"
  else
    log "[FAIL] $name exit=$status,$status2"
    log "----- boot 2 -----"
    cat "$OUT/$name.boot2.serial.log" 2>/dev/null || true
    log "----- expected -----"
    cat "$expected2" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.boot2.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

expect_baddisk() {
  local name="store-baddisk"
  local img="$OUT/$name.img"
  local before="$OUT/$name.before"
  rm -f "$img" "$before" "$OUT/$name.serial.log" "$OUT/$name.qemu.err"
  make_image "$img" badsuper
  cp "$img" "$before"
  local status
  status="$(gate_qemu "$name" 12 "$OUT/task/store-baddisk.steps" "$img")"
  local same=1
  if ! python3 - "$OUT/harness_py.py" "$before" "$img" <<'PY'
import importlib.util, sys
path, left, right = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
sys.exit(0 if mod.same_sha256(open(left, "rb").read(), open(right, "rb").read()) else 1)
PY
  then
    same=0
  fi
  if judge_exact "$status" "$OUT/$name.serial.log" "$SHELL_OK_STATUS" "$OUT/task/store-baddisk.expected" && [[ "$same" -eq 1 ]]; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- expected -----"
    cat "$OUT/task/store-baddisk.expected" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

expect_wasm_core() {
  local name="wasm-core"
  local status
  status="$(run_qemu "$name" 30)"
  if judge_wasm_core "$status" "$OUT/$name.serial.log"; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- expected -----"
    cat "$OUT/task/wasm-core.expected" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
  # Cross-judge the two proofs. A match here is a harness bug, not a case.
  if [[ -f "$OUT/nvme-core.serial.log" ]]; then
    if judge_wasm_core "$NVME_CORE_STATUS" "$OUT/nvme-core.serial.log"; then
      log "[FAIL] swap nvme-core judged as wasm-core"
      FAILURES=$((FAILURES + 1))
    fi
    # The status above is the wrong one, so a content match would still
    # fail the judge. Judge again at the right status: the bytes must differ.
    if judge_wasm_core "$WASM_CORE_STATUS" "$OUT/nvme-core.serial.log"; then
      log "[FAIL] swap nvme-core content judged as wasm-core"
      FAILURES=$((FAILURES + 1))
    fi
    if judge_nvme_core "$WASM_CORE_STATUS" "$OUT/$name.serial.log"; then
      log "[FAIL] swap wasm-core judged as nvme-core"
      FAILURES=$((FAILURES + 1))
    fi
    if judge_nvme_core "$NVME_CORE_STATUS" "$OUT/$name.serial.log"; then
      log "[FAIL] swap wasm-core content judged as nvme-core"
      FAILURES=$((FAILURES + 1))
    fi
    if cmp -s "$OUT/nvme-core.serial.log" "$OUT/$name.serial.log"; then
      log "[FAIL] swap wasm-core and nvme-core logs are identical"
      FAILURES=$((FAILURES + 1))
    fi
  fi
}

expect_wasm_glue() {
  local name="wasm-glue"
  local status golden="$ROOT/apps/glue-duty/golden/wasm-glue.txt"
  status="$(run_qemu "$name" 40)"
  if ! "$ROOT/build/apps/glue-duty-replay" >"$OUT/wasm-glue.replay.txt"; then
    log "[FAIL] $name replay"
    FAILURES=$((FAILURES + 1))
    return
  fi
  if judge_wasm_glue "$status" "$OUT/$name.serial.log" "$OUT/wasm-glue.replay.txt" "$golden"; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- replay -----"
    cat "$OUT/wasm-glue.replay.txt" 2>/dev/null || true
    log "----- golden -----"
    cat "$golden" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
  if judge_wasm_core "$WASM_CORE_STATUS" "$OUT/$name.serial.log"; then
    log "[FAIL] swap wasm-glue judged as wasm-core"
    FAILURES=$((FAILURES + 1))
  fi
}

expect_case() {
  local name="$1"
  local judge="$2"
  local status
  status="$(run_qemu "$name" "$3" "${4:-}")"
  if "$judge" "$status" "$OUT/$name.serial.log"; then
    log "[PASS] $name exit=$status"
  else
    log "[FAIL] $name exit=$status"
    log "----- serial -----"
    cat "$OUT/$name.serial.log" 2>/dev/null || true
    log "----- qemu stderr -----"
    cat "$OUT/$name.qemu.err" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

git_report() {
  local when="$1" head dirty
  head="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  dirty="$(git -C "$ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
  log "git $when head=$head dirty_entries=$dirty"
}

log "toolchain rustc=$(rustc --version)"
log "toolchain cargo=$(cargo --version)"
log "toolchain qemu=$(qemu-system-x86_64 --version | head -n 1)"
git_report before

write_harness_py
selftest_harness

# Use the tracked lockfile only. Do not regenerate it: an online fallback can
# rewrite kernel/Cargo.lock, and `cd && A || B` would run B outside KERNEL_DIR
# when cd fails.
if [[ ! -f "$KERNEL_DIR/Cargo.lock" ]]; then
  log "[FAIL] kernel/Cargo.lock missing; smoke does not create or update it"
  FAILURES=$((FAILURES + 1))
else
  if build_kernel boot-ok; then
    expect_case boot-ok judge_boot_ok 8
  fi
  if build_kernel fault-panic fault-panic; then
    expect_case fault-panic judge_panic 8
  fi
  if build_kernel fault-hang fault-hang; then
    expect_case fault-hang judge_hang 8
  fi
  if build_kernel fault-ud fault-ud; then
    expect_case fault-ud judge_ud 8
  fi
  if build_kernel fault-pf fault-pf; then
    expect_case fault-pf judge_pf 8
  fi
  if build_kernel fault-df fault-df; then
    expect_case fault-df judge_df 8
  fi
  if build_kernel script script; then
    write_script_cases
    expect_script script-ok 8 "$OUT/task/ok.task" "$OUT/task/ok.expected"
    expect_script script-unknown 8 "$OUT/task/unknown.task" "$OUT/task/unknown.expected"
    expect_script script-overflow 8 "$OUT/task/overflow.task" "$OUT/task/overflow.expected"
    expect_script script-repeat 8 "$OUT/task/repeat.task" "$OUT/task/repeat.expected"
    expect_script script-repeat-halt 8 "$OUT/task/repeat-halt.task" "$OUT/task/repeat-halt.expected"
    expect_script script-limit 8 "$OUT/task/limit.task" "$OUT/task/limit.expected"
    expect_script script-nested 8 "$OUT/task/nested.task" "$OUT/task/nested.expected"
    expect_script script-unclosed 8 "$OUT/task/unclosed.task" "$OUT/task/unclosed.expected"
    expect_script script-undefined 8 "$OUT/task/undefined.task" "$OUT/task/undefined.expected"
    expect_script script-syntax 8 "$OUT/task/syntax.task" "$OUT/task/syntax.expected"
    expect_script script-char 8 "$OUT/task/char.task" "$OUT/task/char.expected"
    expect_script script-timeout 20 "$OUT/task/timeout.task" "$OUT/task/timeout.expected" 0
  fi
  write_input_cases
  if build_kernel ticks ticks; then
    expect_case ticks judge_ticks 12
  fi
  if build_kernel keys keys; then
    expect_monitor keys 8 "$OUT/keys.elf" "R3ATH3_KEYS_READY" "a shift-b backspace shift_r-c ret" "$OUT/task/keys.expected" "$KEYS_STATUS"
  fi
  if build_kernel kbd-script kbd-script; then
    expect_monitor kbd-script 8 "$OUT/kbd-script.elf" "R3ATH3_KBD_READY" "s e t spc n spc 1 ret p r i n t spc n ret r u n ret" "$OUT/task/kbd.expected" "$KBD_SCRIPT_STATUS"
  fi
  if build_kernel heap heap; then
    expect_case heap judge_heap 8
  fi
  if build_kernel heap-oom heap-oom; then
    expect_case heap-oom judge_heap_oom 8
  fi
  if build_kernel heap-oom-panic heap-oom-panic; then
    expect_case heap-oom-panic judge_heap_panic 8
  fi
  if build_kernel frames frames; then
    expect_case frames judge_frames 8
  fi
  if build_kernel frames-oom frames-oom; then
    expect_case frames-oom judge_frames_oom 20
  fi
  if build_kernel mem-badinfo mem-badinfo; then
    expect_case mem-badinfo judge_mem_badinfo 8
  fi
  if build_kernel sched-coop sched-coop; then
    expect_case sched-coop judge_sched_coop 12
  fi
  if build_kernel sched-preempt sched-preempt; then
    expect_case sched-preempt judge_sched_preempt 12
  fi
  if build_kernel sched-sleep sched-sleep; then
    expect_case sched-sleep judge_sched_sleep 12
  fi
  if build_kernel sched-full sched-full; then
    expect_case sched-full judge_sched_full 12
  fi
  if build_kernel sched-exit sched-exit; then
    expect_case sched-exit judge_sched_exit 12
  fi
  if build_kernel sched-reap sched-reap; then
    expect_case sched-reap judge_sched_reap 12
  fi
  if build_kernel gate-core gate-core; then
    expect_case gate-core judge_gate_core 12
  fi
  if build_kernel gate-expire gate-expire; then
    expect_case gate-expire judge_gate_expire 12
  fi
  if build_kernel recorder-core recorder-core; then
    expect_case recorder-core judge_recorder_core 12
  fi
  if build_kernel store-core store-core; then
    expect_case store-core judge_store_core 12
  fi
  if build_kernel nvme-core nvme-core; then
    expect_nvme_core
  fi
  if build_kernel wasm-core wasm-core; then
    expect_wasm_core
  fi
  if build_app; then
    if build_kernel wasm-glue wasm-glue; then
      expect_wasm_glue
    fi
    if build_kernel shell shell glue-baked; then
      if write_shell_cases; then
        run_shell_cases 1
      else
        log "[FAIL] write shell cases"
        FAILURES=$((FAILURES + 1))
      fi
    fi
  else
    log "[SKIP] wasm-glue (glue-duty did not build)"
    FAILURES=$((FAILURES + 1))
    if build_kernel shell shell; then
      if write_shell_cases; then
        run_shell_cases 0
      else
        log "[FAIL] write shell cases"
        FAILURES=$((FAILURES + 1))
      fi
    fi
  fi
fi

git_report after
if [[ "$FAILURES" -eq 0 ]]; then
  log "SMOKE PASS"
  exit 0
fi
log "SMOKE FAIL ($FAILURES)"
exit 1
