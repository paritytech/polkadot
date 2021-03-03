#!/bin/bash

# Run a development instance of the Millau Substrate bridge node.

RUST_LOG=runtime=trace \
./target/debug/millau-bridge-node --dev --tmp \
    --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
    --port 33044 --rpc-port 9934 --ws-port 9945 \
