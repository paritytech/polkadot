#!/bin/bash

# Runs all benchmarks for all pallets, for each of the runtimes specified below
# Should be run on a reference machine to gain accurate benchmarks
# current reference machine: https://github.com/paritytech/substrate/pull/5848

runtimes=(
  polkadot
  kusama
  westend
)

# cargo build --locked --release
for runtime in "${runtimes[@]}"; do
  cargo +nightly run --release --features=runtime-benchmarks --locked benchmark --chain "${runtime}-dev" --execution=wasm --wasm-execution=compiled --pallet "*" --extrinsic "*" --repeat 0 | sed -r -e 's/Pallet: "([a-z_:]+)".*/\1/' | uniq | grep -v frame_system > "${runtime}_pallets"
  while read -r line; do
    pallet="$(echo "$line" | cut -d' ' -f1)";
    echo "Runtime: $runtime. Pallet: $pallet";
    cargo +nightly run --release --features=runtime-benchmarks -- benchmark --chain="${runtime}-dev" --steps=50 --repeat=20 --pallet="$pallet" --extrinsic="*" --execution=wasm --wasm-execution=compiled --heap-pages=4096 --header=./file_header.txt --output="./runtime/${runtime}/src/weights/${pallet/::/_}.rs"
  done < "${runtime}_pallets"
  rm "${runtime}_pallets"
done
