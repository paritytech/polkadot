#!/bin/bash
set -xeu

sleep 3
curl -v http://millau-node-bob:9933/health
curl -v http://rialto-node-bob:9933/health

MESSAGE_LANE=${MSG_EXCHANGE_GEN_LANE:-00000000}

/home/user/substrate-relay relay-messages RialtoToMillau \
	--lane $MESSAGE_LANE \
	--source-host rialto-node-bob \
	--source-port 9944 \
	--source-signer //Ferdie \
	--target-host millau-node-bob \
	--target-port 9944 \
	--target-signer //Ferdie \
	--prometheus-host=0.0.0.0
