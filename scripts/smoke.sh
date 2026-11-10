#!/usr/bin/env bash
# Bounded QEMU boot smoke test. Exits 0 only when every case passes.
# QEMU isa-debug-exit maps port 0xF4 value V to process status (V << 1) | 1.
# Success writes 0x10 (status 33). A panic writes 0x11 (status 35).
# CPU faults: #UD 0x31→99, #PF 0x33→103, #DF 0x34→105.
# Any other vector writes 0x2F (status 95), outside timeout's 124–127.
# Task scripts write 0x50 (ok → 161) or 0x51 (error → 163).
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
}

run_script_qemu() {
  local name="$1"
  local seconds="$2"
  local script_file="$3"
  local elf="$OUT/script.elf"
  local log_file="$OUT/$name.serial.log"
  local err_file="$OUT/$name.qemu.err"
  if [[ ! -f "$elf" ]]; then
    echo "missing elf" >"$log_file"
    echo 127
    return
  fi
  set +e
  timeout --foreground --kill-after=5s "$seconds" python3 - "$script_file" "$elf" "$log_file" "$err_file" "$seconds" <<'PY'
import os, select, signal, subprocess, sys, time
script_path, elf, out_log, err_log, seconds = sys.argv[1:]
payload = open(script_path, "rb").read()
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
        for start in range(0, len(view), 8):
            proc.stdin.write(view[start:start + 8])
            proc.stdin.flush()
            time.sleep(0.001)
        sent = True
    if proc.poll() is not None:
        leftover = os.read(fd, 4096) if ready else b""
        if leftover:
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
  local want="$SCRIPT_OK_STATUS"
  case "$name" in
    script-ok) want="$SCRIPT_OK_STATUS" ;;
    *) want="$SCRIPT_ERR_STATUS" ;;
  esac
  local status
  status="$(run_script_qemu "$name" "$seconds" "$script_file")"
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

log "toolchain rustc=$(rustc --version)"
log "toolchain qemu=$(qemu-system-x86_64 --version | head -n 1)"

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
  fi
fi

if [[ "$FAILURES" -eq 0 ]]; then
  log "SMOKE PASS"
  exit 0
fi
log "SMOKE FAIL ($FAILURES)"
exit 1
