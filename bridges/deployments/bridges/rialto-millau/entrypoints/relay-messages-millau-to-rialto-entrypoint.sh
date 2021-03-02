#!/bin/bash
set -xeu

sleep 3
curl -v http://millau-node-bob:9933/health
curl -v http://rialto-node-bob:9933/health

MESSAGE_LANE=${MSG_EXCHANGE_GEN_LANE:-00000000}

/home/user/substrate-relay relay-messages millau-to-rialto \
	--lane $MESSAGE_LANE \
	--millau-host millau-node-bob \
	--millau-port 9944 \
	--millau-signer //Eve \
	--rialto-host rialto-node-bob \
	--rialto-port 9944 \
	--rialto-signer //Eve \
	--prometheus-host=0.0.0.0
