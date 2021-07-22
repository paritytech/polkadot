#!/bin/bash

# Runs all benchmarks for all pallets, for a given runtime, provided by $1
# Should be run on a reference machine to gain accurate benchmarks
# current reference machine: https://github.com/paritytech/substrate/pull/5848

runtime="$1"

echo "[+] Running all benchmarks for $runtime"

cargo +nightly run --release --features=runtime-benchmarks --locked benchmark --chain "${runtime}-dev" --execution=wasm --wasm-execution=compiled --pallet "*" --extrinsic "*" --repeat 0 | sed -r -e 's/Pallet: "([a-z_:]+)".*/\1/' | uniq | grep -v frame_system > "${runtime}_pallets"
while read -r line; do
  pallet="$(echo "$line" | cut -d' ' -f1)";
  echo "Runtime: $runtime. Pallet: $pallet";
  cargo +nightly run --release --features=runtime-benchmarks -- benchmark --chain="${runtime}-dev" --steps=50 --repeat=20 --pallet="$pallet" --extrinsic="*" --execution=wasm --wasm-execution=compiled --heap-pages=4096 --header=./file_header.txt --output="./runtime/${runtime}/src/weights/${pallet/::/_}.rs"
done < "${runtime}_pallets"
rm "${runtime}_pallets"
