#!/bin/bash

# Run a development instance of the Westend Substrate bridge node.
# To override the default port just export WESTEND_PORT=9945
#
# Note: This script will not work out of the box with the bridges
# repo since it relies on a Polkadot binary.

WESTEND_PORT="${WESTEND_PORT:-9944}"

RUST_LOG=runtime=trace,runtime::bridge=trace \
./target/debug/polkadot --chain=westend-dev --alice --tmp \
    --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
    --port 33033 --rpc-port 9933 --ws-port $WESTEND_PORT \
