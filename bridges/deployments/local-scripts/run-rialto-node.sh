#!/bin/bash

# Run a development instance of the Rialto Substrate bridge node.

RUST_LOG=runtime=trace \
    ./target/debug/rialto-bridge-node --dev --tmp \
    --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
    --port 33033 --rpc-port 9933 --ws-port 9944 \
