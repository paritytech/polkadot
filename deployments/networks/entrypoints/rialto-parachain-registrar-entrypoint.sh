#!/bin/bash
set -xeu

sleep 15

/home/user/substrate-relay register-parachain rialto-parachain \
	--parachain-host rialto-parachain-collator-alice \
	--parachain-port 9944 \
	--relaychain-host rialto-node-alice \
	--relaychain-port 9944 \
	--relaychain-signer //Alice
