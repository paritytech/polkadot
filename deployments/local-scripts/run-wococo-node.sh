#!/bin/bash

# Run a development instance of the Wococo Substrate bridge node.
# To override the default port just export WOCOCO_PORT=9955
#
# Note: This script will not work out of the box with the bridges
# repo since it relies on a Polkadot binary.

WOCOCO_PORT="${WOCOCO_PORT:-9944}"

RUST_LOG=runtime=trace,runtime::bridge=trace \
./target/debug/polkadot --chain=wococo-dev --alice --tmp \
    --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
    --port 33033 --rpc-port 9933 --ws-port $WOCOCO_PORT \
