#!/bin/bash
set -xeu

sleep 3
curl -v http://poa-node-arthur:8545/api/health
curl -v http://poa-node-bertha:8545/api/health
curl -v http://poa-node-carlos:8545/api/health
curl -v http://rialto-node-alice:9933/health
curl -v http://rialto-node-bob:9933/health
curl -v http://rialto-node-charlie:9933/health

/home/user/ethereum-poa-relay eth-exchange-sub \
	--sub-host rialto-node-alice \
	--sub-signer //Bob \
	--eth-host poa-node-arthur \
	--prometheus-host=0.0.0.0
