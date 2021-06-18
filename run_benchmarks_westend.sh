#!/bin/bash

steps=50
repeat=20

output=./runtime/westend/src/weights/
chain=westend-dev

pallets=(
	runtime_common::auctions
	runtime_common::crowdloan
	runtime_common::paras_registrar
	runtime_common::slots
	pallet_balances
	pallet_election_provider_multi_phase
	pallet_identity
	pallet_im_online
	pallet_indices
	pallet_multisig
	pallet_offences
	pallet_proxy
	pallet_scheduler
	pallet_session
	pallet_staking
	frame_system
	pallet_timestamp
	pallet_utility
	pallet_vesting
)

for p in ${pallets[@]}
do
	target/release/polkadot benchmark \
		--chain=$chain \
		--steps=$steps  \
		--repeat=$repeat \
		--pallet=$p  \
		--extrinsic='*' \
		--execution=wasm \
		--wasm-execution=compiled \
		--heap-pages=4096 \
		--header=./file_header.txt \
		--output=$output

done
