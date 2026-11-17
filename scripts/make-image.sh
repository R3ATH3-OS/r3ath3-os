#!/bin/sh
# Build the hardware-safe USB image. Writes one file, never a device.
# Output: build/image/r3ath3-usb.img
# Prints its SHA-256 and its size in bytes.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/build/image/r3ath3-usb.img"
STAGE="$ROOT/build/image/iso"

case "$OUT" in
  /dev/*)
    echo "refusing to write a device: $OUT" >&2
    exit 1
    ;;
esac
if [ -b "$OUT" ]; then
  echo "refusing to write a block device: $OUT" >&2
  exit 1
fi
for tool in cargo grub-mkrescue xorriso; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing tool: $tool" >&2
    exit 1
  fi
done

"$ROOT/scripts/build-apps.sh"
(cd "$ROOT/kernel" && cargo build --release --offline --features shell,glue-baked,hw)

rm -rf "$STAGE"
mkdir -p "$STAGE/boot/grub" "$ROOT/build/image"
cp "$ROOT/kernel/target/x86_64-unknown-none/release/r3ath3" "$STAGE/boot/r3ath3"
cp "$ROOT/scripts/grub.cfg" "$STAGE/boot/grub/grub.cfg"
grub-mkrescue -o "$OUT" "$STAGE"
if [ -b "$OUT" ]; then
  echo "refusing to leave a block device at $OUT" >&2
  exit 1
fi
sha256sum "$OUT"
stat -c 'size=%s' "$OUT"
