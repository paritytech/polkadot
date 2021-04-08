#!/bin/bash
set -xeu

sleep 3
curl -v http://millau-node-alice:9933/health
curl -v http://rialto-node-alice:9933/health

/home/user/substrate-relay init-bridge millau-to-rialto \
	--millau-host millau-node-alice \
	--millau-port 9944 \
	--rialto-host rialto-node-alice \
	--rialto-port 9944 \
	--rialto-signer //Alice

# Give chain a little bit of time to process initialization transaction
sleep 6
/home/user/substrate-relay relay-headers millau-to-rialto \
	--millau-host millau-node-alice \
	--millau-port 9944 \
	--rialto-host rialto-node-alice \
	--rialto-port 9944 \
	--rialto-signer //Charlie \
	--prometheus-host=0.0.0.0
