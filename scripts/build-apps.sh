#!/bin/sh
# Build Glue Duty for the interpreter and for the native replay.
# The kernel bakes build/apps/glue_duty.wasm. Nothing here is committed.
set -eu
cd "$(dirname "$0")/../apps/glue-duty"
cargo build --release --offline --target wasm32-unknown-unknown --lib
cargo build --release --offline --bin replay
mkdir -p ../../build/apps
cp target/wasm32-unknown-unknown/release/glue_duty.wasm ../../build/apps/
cp target/release/replay ../../build/apps/glue-duty-replay
