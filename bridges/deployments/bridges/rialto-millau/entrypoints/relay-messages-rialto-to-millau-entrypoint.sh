#!/bin/bash
set -xeu

sleep 3
curl -v http://millau-node-bob:9933/health
curl -v http://rialto-node-bob:9933/health

MESSAGE_LANE=${MSG_EXCHANGE_GEN_LANE:-00000000}

/home/user/substrate-relay relay-messages rialto-to-millau \
	--lane $MESSAGE_LANE \
	--rialto-host rialto-node-bob \
	--rialto-port 9944 \
	--rialto-signer //Ferdie \
	--millau-host millau-node-bob \
	--millau-port 9944 \
	--millau-signer //Ferdie \
	--prometheus-host=0.0.0.0
