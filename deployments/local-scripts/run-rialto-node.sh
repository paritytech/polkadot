#!/bin/bash

# Run a development instance of the Rialto Substrate bridge node.
# To override the default port just export RIALTO_PORT=9944

RIALTO_PORT="${RIALTO_PORT:-9944}"

RUST_LOG=runtime=trace \
    ./target/debug/rialto-bridge-node --dev --tmp \
    --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
    --port 33033 --rpc-port 9933 --ws-port $RIALTO_PORT \
