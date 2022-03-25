#!/bin/bash
set -xeu

sleep 15

/home/user/substrate-relay init-bridge millau-to-rialto \
	--source-host millau-node-alice \
	--source-port 9944 \
	--target-host rialto-node-alice \
	--target-port 9944 \
	--target-signer //Alice

/home/user/substrate-relay init-bridge rialto-to-millau \
	--source-host rialto-node-alice \
	--source-port 9944 \
	--target-host millau-node-alice \
	--target-port 9944 \
	--target-signer //Alice

# Give chain a little bit of time to process initialization transaction
sleep 6

/home/user/substrate-relay relay-headers-and-messages millau-rialto \
	--millau-host millau-node-alice \
	--millau-port 9944 \
	--millau-signer //Charlie \
	--millau-messages-pallet-owner=//RialtoMessagesOwner \
	--millau-transactions-mortality=64 \
	--rialto-host rialto-node-alice \
	--rialto-port 9944 \
	--rialto-signer //Charlie \
	--rialto-messages-pallet-owner=//MillauMessagesOwner \
	--rialto-transactions-mortality=64 \
	--lane=00000000 \
	--lane=73776170 \
	--prometheus-host=0.0.0.0
