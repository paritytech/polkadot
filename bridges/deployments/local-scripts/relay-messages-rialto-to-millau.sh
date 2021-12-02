#!/bin/bash
# A script for relaying Rialto messages to the Millau chain.
#
# Will not work unless both the Rialto and Millau are running (see `run-rialto-node.sh`
# and `run-millau-node.sh).
set -xeu

MILLAU_PORT="${MILLAU_PORT:-9945}"
RIALTO_PORT="${RIALTO_PORT:-9944}"

RUST_LOG=bridge=debug \
./target/debug/substrate-relay relay-messages rialto-to-millau \
	--lane 00000000 \
	--source-host localhost \
	--source-port $RIALTO_PORT \
	--source-signer //Bob \
	--target-host localhost \
	--target-port $MILLAU_PORT \
	--target-signer //Bob \
	--prometheus-host=0.0.0.0
