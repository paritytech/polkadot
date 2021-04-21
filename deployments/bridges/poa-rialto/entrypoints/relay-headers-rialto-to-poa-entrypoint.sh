#!/bin/bash
set -xeu

sleep 10

curl -v http://rialto-node-bob:9933/health
curl -v http://poa-node-bertha:8545/api/health

# Try to deploy contracts first
# networkID = 0x69
# Arthur's key.
/home/user/ethereum-poa-relay eth-deploy-contract \
	--eth-chain-id 105 \
	--eth-signer 0399dbd15cf6ee8250895a1f3873eb1e10e23ca18e8ed0726c63c4aea356e87d \
	--sub-host rialto-node-bob \
	--eth-host poa-node-bertha || echo "Failed to deploy contracts."

sleep 10
echo "Starting SUB -> ETH relay"
/home/user/ethereum-poa-relay sub-to-eth \
	--eth-contract c9a61fb29e971d1dabfd98657969882ef5d0beee \
	--eth-chain-id 105 \
	--eth-signer 0399dbd15cf6ee8250895a1f3873eb1e10e23ca18e8ed0726c63c4aea356e87d \
	--sub-host rialto-node-bob \
	--eth-host poa-node-bertha \
	--prometheus-host=0.0.0.0
