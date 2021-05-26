#!/bin/bash

# A script for relaying Rialto headers to the Millau chain.
#
# Will not work unless both the Rialto and Millau are running (see `run-rialto-node.sh`
# and `run-millau-node.sh).

MILLAU_PORT="${MILLAU_PORT:-9945}"
RIALTO_PORT="${RIALTO_PORT:-9944}"

RUST_LOG=bridge=debug \
./target/debug/substrate-relay init-bridge RialtoToMillau \
	--target-host localhost \
	--target-port $MILLAU_PORT \
	--source-host localhost \
	--source-port $RIALTO_PORT \
	--target-signer //Alice \

sleep 5
RUST_LOG=bridge=debug \
./target/debug/substrate-relay relay-headers RialtoToMillau \
	--target-host localhost \
	--target-port $MILLAU_PORT \
	--source-host localhost \
	--source-port $RIALTO_PORT \
	--target-signer //Alice \
	--prometheus-host=0.0.0.0
