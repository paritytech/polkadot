#!/usr/bin/env bash
set -e

# Make LLD produce a binary that imports memory from the outside environment.
export RUSTFLAGS="-C link-arg=--import-memory -C link-arg=--export-table -C panic=abort"

if cargo --version | grep -q "nightly"; then
	CARGO_CMD="cargo"
else
	CARGO_CMD="cargo +nightly"
fi

for i in adder
do
	cd $i/wasm
	$CARGO_CMD build --target=wasm32-unknown-unknown --release --no-default-features --target-dir target "$@"
	wasm-gc target/wasm32-unknown-unknown/release/$i'_'wasm.wasm target/wasm32-unknown-unknown/release/$i.wasm
	cp target/wasm32-unknown-unknown/release/$i.wasm ../../../parachain/tests/res/
	rm -rf target
	cd ../..
done
