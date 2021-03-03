#!/bin/bash

# This script assumes that an OpenEthereum build is available. The repo
# should be at the same level as the `parity-bridges-common` repo.

RUST_LOG=rpc=trace,txqueue=trace,bridge-builtin=trace \
../openethereum/target/debug/openethereum \
    --config="$(pwd)"/deployments/dev/poa-config/poa-node-config \
    --node-key=arthur \
    --engine-signer=0x005e714f896a8b7cede9d38688c1a81de72a58e4 \
    --base-path=/tmp/oe-dev-node \
