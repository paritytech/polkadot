#!/bin/bash
set -xeu

sleep 60
curl -v http://rialto-node-alice:9933/health
curl -v http://rialto-parachain-collator-alice:9933/health

/home/user/substrate-relay register-parachain rialto-parachain \
	--parachain-host rialto-parachain-collator-alice \
	--parachain-port 9944 \
	--relaychain-host rialto-node-alice \
	--relaychain-port 9944 \
	--relaychain-signer //Alice
