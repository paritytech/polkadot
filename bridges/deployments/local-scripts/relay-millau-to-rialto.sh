#!/bin/bash

# A script for relaying Millau headers to the Rialto chain.
#
# Will not work unless both the Rialto and Millau are running (see `run-rialto-node.sh`
# and `run-millau-node.sh).

RUST_LOG=bridge=debug \
./target/debug/substrate-relay init-bridge millau-to-rialto \
	--millau-host localhost \
	--millau-port 9945 \
	--rialto-host localhost \
	--rialto-port 9944 \
	--rialto-signer //Alice \

sleep 5
RUST_LOG=bridge=debug \
./target/debug/substrate-relay relay-headers millau-to-rialto \
	--millau-host localhost \
	--millau-port 9945 \
	--rialto-host localhost \
	--rialto-port 9944 \
	--rialto-signer //Alice \
	--prometheus-host=0.0.0.0
