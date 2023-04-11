#!/bin/bash

cargo build --release && \
cp target/release/polkadot . && \
podman build -t docker.io/nielswps/relay-chain-node:latest -f Dockerfile && \
podman push nielswps/relay-chain-node && \

podman build -t docker.io/nielswps/relay-chain-node-init:latest -f InitContainerDockerfile && \
podman push nielswps/relay-chain-node-init && \

#./parachain-energychain-node build-spec --disable-default-bootnode --chain=local > energychain-spec.json && \
./polkadot build-spec --disable-default-bootnode --chain=relay-chain-spec.json --raw > raw-relay-chain-spec.json && \
cp raw-relay-chain-spec.json ../polkem/chain-specs/relay-chain-spec.json && \
rm polkadot