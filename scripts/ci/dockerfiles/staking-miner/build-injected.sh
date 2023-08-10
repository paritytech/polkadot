#!/usr/bin/env bash

# Sample call:
# $0 /path/to/folder_with_staking-miner_binary
# This script replace the former dedicated staking-miner "injected" Dockerfile
# and shows how to use the generic binary_injected.dockerfile

PROJECT_ROOT=`git rev-parse --show-toplevel`

export BINARY=staking-miner
export BIN_FOLDER=$1

$PROJECT_ROOT/scripts/ci/dockerfiles/build-injected.sh
