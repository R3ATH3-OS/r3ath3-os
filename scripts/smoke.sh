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
  rm -f "$sock"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - "$elf" "$log_file" "$err_file" "$seconds" "$ready" "$keys" "$sock" <<'PY'
import os, select, socket, subprocess, sys, time
elf, out_log, err_log, seconds, ready, keys, sock_path = sys.argv[1:]
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
  rm -f "$sock" "$vga_file"
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - \
    "$elf" "$log_file" "$err_file" "$seconds" "$steps" "$sock" "$vga_file" <<'PY'
import os, re, signal, socket, subprocess, sys, time
elf, out_log, err_log, seconds, steps_path, sock_path, vga_path = sys.argv[1:]
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

def prompt_count():
    return read_serial().count(b"r3> ")

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
        steps.append(("line", qcodes_for_text("")))
    elif raw.startswith("line "):
        steps.append(("line", qcodes_for_text(raw[5:])))
    elif raw.startswith("keys "):
        steps.append(("keys", raw[5:].split()))
    elif raw == "xp":
        steps.append(("xp", []))
    else:
        fail(2, "bad step " + raw)

ready_deadline = min(deadline, time.time() + 5)
while time.time() < ready_deadline:
    if proc.poll() is not None:
        break
    data = read_serial()
    if b"R3ATH3_SHELL_READY" in data and prompt_count() >= 1:
        break
    time.sleep(0.02)
else:
    fail(124, "timed out waiting for the shell prompt")
if b"R3ATH3_SHELL_READY" not in read_serial() or prompt_count() < 1:
    fail(124, "shell prompt missing")

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

def wait_prompts(target):
    limit = time.time() + 5
    while time.time() < limit and time.time() < deadline:
        if prompt_count() >= target:
            return True
        if proc.poll() is not None:
            return False
        time.sleep(0.02)
    return False

for index, (kind, keys) in enumerate(steps):
    last = index + 1 == len(steps)
    if kind in ("line", "keys"):
        before = prompt_count()
        if not send_keys(keys):
            fail(1, "monitor closed while sending keys")
        if not last and not wait_prompts(before + 1):
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
