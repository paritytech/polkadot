#!/bin/bash
set -xeu

sleep 3
curl -v http://millau-node-alice:9933/health
curl -v https://westend-rpc.polkadot.io:443/health

/home/user/substrate-relay init-bridge WestendToMillau \
	--source-host westend-rpc.polkadot.io \
	--source-port 443 \
	--source-secure \
	--target-host millau-node-alice \
	--target-port 9944 \
	--target-signer //George

# Give chain a little bit of time to process initialization transaction
sleep 6
/home/user/substrate-relay relay-headers WestendToMillau \
	--source-host westend-rpc.polkadot.io \
	--source-port 443 \
	--source-secure \
	--target-host millau-node-alice \
	--target-port 9944 \
	--target-signer //George \
	--prometheus-host=0.0.0.0
