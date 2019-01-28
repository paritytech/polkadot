#!/usr/bin/env bash
set -e

if cargo --version | grep -q "nightly"; then
	CARGO_CMD="cargo"
else
	CARGO_CMD="cargo +nightly"
fi
RUSTFLAGS="-C link-arg=--export-table" $CARGO_CMD build --target=wasm32-unknown-unknown --release
for i in polkadot_runtime
do
	wasm-gc target/wasm32-unknown-unknown/release/$i.wasm target/wasm32-unknown-unknown/release/$i.compact.wasm
done
