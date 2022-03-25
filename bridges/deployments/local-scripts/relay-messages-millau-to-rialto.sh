#!/bin/bash
# A script for relaying Millau messages to the Rialto chain.
#
# Will not work unless both the Rialto and Millau are running (see `run-rialto-node.sh`
# and `run-millau-node.sh).
set -xeu

MILLAU_PORT="${MILLAU_PORT:-9945}"
RIALTO_PORT="${RIALTO_PORT:-9944}"

RUST_LOG=bridge=debug \
./target/debug/substrate-relay relay-messages millau-to-rialto \
	--relayer-mode=altruistic \
	--lane 00000000 \
	--source-host localhost \
	--source-port $MILLAU_PORT \
	--source-signer //Bob \
	--target-host localhost \
	--target-port $RIALTO_PORT \
	--target-signer //Bob \
	--prometheus-host=0.0.0.0
