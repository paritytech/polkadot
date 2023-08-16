#!/usr/bin/env bash

# Sample call:
# $0 /path/to/folder_with_staking-miner_binary
# This script replace the former dedicated staking-miner "injected" Dockerfile
# and shows how to use the generic binary_injected.dockerfile

PROJECT_ROOT=`git rev-parse --show-toplevel`
ENGINE=podman

echo "Building the staking-miner using the Builder image"
echo "PROJECT_ROOT=$PROJECT_ROOT"
$ENGINE build -t staking-miner -f staking-miner_builder.Dockerfile "$PROJECT_ROOT"
