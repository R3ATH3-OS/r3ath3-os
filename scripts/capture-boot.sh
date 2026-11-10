#!/usr/bin/env bash
# One QEMU VGA screendump of the boot marker. Not part of the pass/fail smoke test.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL_DIR="$ROOT/kernel"
OUT="$ROOT/build/capture"
SOCK="$OUT/monitor.sock"
mkdir -p "$OUT"
rm -f "$SOCK" "$OUT/boot.ppm"

(cd "$KERNEL_DIR" && cargo build --release --offline --features capture)

cp "$KERNEL_DIR/target/x86_64-unknown-none/release/r3ath3" "$OUT/capture.elf"

qemu-system-x86_64 \
  -machine pc,accel=tcg \
  -cpu qemu64 \
  -m 64M \
  -smp 1 \
  -kernel "$OUT/capture.elf" \
  -nic none \
  -display vnc=127.0.0.1:7 \
  -vga std \
  -serial file:"$OUT/serial.log" \
  -no-reboot \
  -monitor unix:"$SOCK",server,nowait \
  >/dev/null 2>"$OUT/qemu.err" &
QEMU_PID=$!

python3 - "$SOCK" "$OUT/boot.ppm" "$QEMU_PID" <<'PY'
import socket, sys, time, os, signal
sock_path, ppm_path, pid = sys.argv[1], sys.argv[2], int(sys.argv[3])
deadline = time.time() + 8
while not os.path.exists(sock_path):
    if time.time() > deadline:
        raise SystemExit("monitor socket did not appear")
    time.sleep(0.05)
time.sleep(0.4)
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(5)
client.connect(sock_path)
client.recv(4096)
client.sendall(f"screendump {ppm_path}\n".encode())
deadline = time.time() + 5
while not os.path.exists(ppm_path):
    if time.time() > deadline:
        raise SystemExit("screendump file was not written")
    time.sleep(0.05)
client.sendall(b"quit\n")
client.close()
try:
    os.kill(pid, 0)
except OSError:
    pass
else:
    deadline = time.time() + 5
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except OSError:
            break
        time.sleep(0.05)
    else:
        os.kill(pid, signal.SIGKILL)
PY

wait "$QEMU_PID" || true

python3 - "$OUT/boot.ppm" "$OUT/boot.png" <<'PY'
import struct, sys, zlib
ppm_path, png_path = sys.argv[1], sys.argv[2]
data = open(ppm_path, "rb").read()
if not data.startswith(b"P6"):
    raise SystemExit("screendump was not a P6 PPM")
header, pixel_start = b"", 0
idx = 0
tokens = []
while len(tokens) < 4:
    if idx >= len(data):
        raise SystemExit("truncated PPM header")
    if data[idx] in b" \t\r\n":
        if header:
            tokens.append(header)
            header = b""
        idx += 1
        continue
    if data[idx] == ord("#"):
        while idx < len(data) and data[idx] not in b"\r\n":
            idx += 1
        continue
    header += data[idx:idx+1]
    idx += 1
width, height, depth = int(tokens[1]), int(tokens[2]), int(tokens[3])
if depth != 255:
    raise SystemExit(f"unexpected PPM depth {depth}")
pixels = data[idx:]
stride = width * 3
if len(pixels) < height * stride:
    raise SystemExit("truncated PPM pixels")
raw = b"".join(b"\x00" + pixels[y*stride:(y+1)*stride] for y in range(height))

def chunk(tag, payload):
    crc = zlib.crc32(tag + payload) & 0xFFFFFFFF
    return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", crc)

ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
open(png_path, "wb").write(png)
print(f"wrote {png_path} {width}x{height}")
PY

if ! grep -F -x -q "R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation" "$OUT/serial.log"; then
  echo "capture serial log missing boot marker" >&2
  cat "$OUT/serial.log" >&2 || true
  exit 1
fi
echo "CAPTURE PASS"
