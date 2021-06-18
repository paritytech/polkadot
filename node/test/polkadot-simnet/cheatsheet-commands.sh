#!/bin/bash
# Build docker image with by copying the binary into the image
# First you need to copy the binary to this dire because all files in
# polkadot/target are ignored because they are specified in .dockerignore file

# cp ../../../target/release/polkadot-simnet .
# docker build -t paritypr/forrestgump:"$1" -f build-bin-remote.Dockerfile .
# docker push paritypr/forrestgump:"$1"

# build docker image locally using cache from cargo-chef
# add export DOCKER_BUILDKIT=1  to ~/.bashrc
docker build -t seunlanlege/forrestgump:"$1" -f local-build-with-cargo-chef.Dockerfile --no-cache ../../..
docker push seunlanlege/forrestgump:"$1"
