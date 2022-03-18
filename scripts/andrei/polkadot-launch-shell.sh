#!/bin/bash
ROOT=$(git rev-parse --show-toplevel)

podman run -it \
    -v $ROOT/target/release/polkadot:/polkadot/polkadot \
    -v $ROOT/target/release/adder-collator:/polkadot-collator/adder-collator \
    --network host \
    polkadot-launch /bin/bash