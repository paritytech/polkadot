#!/bin/sh
#
# Runtime benchmarks for the `pallet-bridge-messages` and `pallet-bridge-grandpa` pallets.
#
# Run this script from root of the repo.

set -eux

time cargo run --release -p millau-bridge-node --features=runtime-benchmarks -- benchmark \
	--chain=dev \
	--steps=50 \
	--repeat=20 \
	--pallet=pallet_bridge_messages \
	--extrinsic=* \
	--execution=wasm \
	--wasm-execution=Compiled \
	--heap-pages=4096 \
	--output=./modules/messages/src/weights.rs \
	--template=./.maintain/millau-weight-template.hbs

time cargo run --release -p millau-bridge-node --features=runtime-benchmarks -- benchmark \
	--chain=dev \
	--steps=50 \
	--repeat=20 \
	--pallet=pallet_bridge_grandpa \
	--extrinsic=* \
	--execution=wasm \
	--wasm-execution=Compiled \
	--heap-pages=4096 \
	--output=./modules/grandpa/src/weights.rs \
	--template=./.maintain/millau-weight-template.hbs

time cargo run --release -p millau-bridge-node --features=runtime-benchmarks -- benchmark \
	--chain=dev \
	--steps=50 \
	--repeat=20 \
	--pallet=pallet_bridge_token_swap \
	--extrinsic=* \
	--execution=wasm \
	--wasm-execution=Compiled \
	--heap-pages=4096 \
	--output=./modules/token-swap/src/weights.rs \
	--template=./.maintain/millau-weight-template.hbs
