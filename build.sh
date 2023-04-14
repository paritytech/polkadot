#!/bin/bash

cargo build --release && \
cp target/release/polkadot . && \
podman build -t docker.io/nielswps/relay-chain-node:latest -f Dockerfile && \
podman push nielswps/relay-chain-node && \

#./polkadot build-spec --disable-default-bootnode --chain=rococo-local > relay-chain-spec.json && \
#./polkadot build-spec --disable-default-bootnode --chain=relay-chain-spec.json --raw > raw-relay-chain-spec.json && \
#cp raw-relay-chain-spec.json ../polkem/chain-specs/relay-chain-spec.json && \
rm polkadot
