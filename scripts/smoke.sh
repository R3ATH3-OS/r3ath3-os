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
# Unused odd statuses: 203–207, 219–223, 237–239, 245–255.
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

judge_exact() {
  local status="$1" file="$2" expected_status="$3" expected_file="$4"
  [[ "$status" -eq "$expected_status" ]] || return 1
  cmp -s "$file" "$expected_file"
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

run_qemu() {
  local name="$1"
  local seconds="$2"
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
    "run SCRIPT  run a task script, lines separated by ;\n"
    "exit        leave the shell with the last status\n"
    "halt        stop the machine\n"
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
      if judge_shell_mem "$status" "$log_file" "$expected"; then
        passed=1
      fi
      ;;
    screen)
      if judge_shell_screen "$status" "$log_file" "$expected" "$vga_file" "$OUT/task/shell-screen.vga"; then
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
    if [[ "$kind" == "screen" ]]; then
      log "----- vga -----"
      cat "$vga_file" 2>/dev/null || true
    fi
    log "----- qemu stderr -----"
    cat "$err_file" 2>/dev/null || true
    FAILURES=$((FAILURES + 1))
  fi
}

expect_gate() {
  local name="$1"
  local seconds="$2"
  local steps="$3"
  local expected="$4"
  local want="$5"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  local sock="$OUT/$name.mon.sock"
  local ser="$OUT/$name.ser.sock"
  local elf="$OUT/shell.elf"
  rm -f "$sock" "$ser" "$log_file"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - \
    "$elf" "$log_file" "$err_file" "$seconds" "$steps" "$sock" "$ser" "$OUT/harness_py.py" <<'PY'
import importlib.util, os, signal, socket, subprocess, sys, time
elf, out_log, err_log, seconds, steps_path, sock_path, ser_path, harness_path = sys.argv[1:]
spec = importlib.util.spec_from_file_location("harness_py", harness_path)
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
for path in (out_log, ser_path, sock_path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass
err_f = open(err_log, "wb")
proc = subprocess.Popen(
    [
        "qemu-system-x86_64", "-machine", "pc,accel=tcg", "-cpu", "qemu64",
        "-m", "64M", "-smp", "1", "-kernel", elf, "-nic", "none",
        "-display", "none", "-vga", "std",
        "-chardev", "socket,id=com1,path=" + ser_path + ",server=on,wait=on",
        "-serial", "chardev:com1",
        "-monitor", "unix:" + sock_path + ",server,nowait",
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ],
    stdout=subprocess.DEVNULL, stderr=err_f,
)
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

expect_case() {
  local name="$1"
  local judge="$2"
  local status
  status="$(run_qemu "$name" "$3")"
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
  if build_kernel shell shell; then
    if write_shell_cases; then
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
      expect_shell shell-sched-args 12 "$OUT/task/shell-sched-args.steps" "$OUT/task/shell-sched-args.expected" "$SHELL_CMD_STATUS" exact
    else
      log "[FAIL] write shell cases"
      FAILURES=$((FAILURES + 1))
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
