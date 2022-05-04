#!/bin/bash

set -exu

# Set up the standardized machine and run `update-weights.sh` script.
# The system is assumed to be pristine Ubuntu 20.04 and we install
# all required dependencies.

# To avoid interruptions you might want to run this script in `screen` cause it will take a while
# to finish.

# We start off with upgrading the system
apt update && apt dist-upgrade

# and installing `git` and other required deps.
apt install -y git clang curl libssl-dev llvm libudev-dev screen

# Now we clone the repository
git clone https://github.com/paritytech/parity-bridges-common.git
cd parity-bridges-common

# Install rustup & toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y

# Source config
source ~/.cargo/env

# Add nightly and WASM
rustup install nightly
rustup target add wasm32-unknown-unknown --toolchain nightly

# Update the weights
./scripts/update-weights.sh
