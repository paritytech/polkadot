#!/bin/bash

steps=50
repeat=20

output=./runtime/kusama/src/weights/
chain=kusama-dev

pallets=(
	runtime_common::auctions
	runtime_common::crowdloan
	runtime_common::claims
	runtime_common::slots
	runtime_common::paras_registrar
	pallet_balances
	pallet_bounties
	pallet_collective
	pallet_democracy
	pallet_elections_phragmen
	pallet_election_provider_multi_phase
	pallet_gilt
	pallet_identity
	pallet_im_online
	pallet_indices
	pallet_membership
	pallet_multisig
	pallet_offences
	pallet_proxy
	pallet_scheduler
	pallet_session
	pallet_staking
	frame_system
	pallet_timestamp
	pallet_tips
	pallet_treasury
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
