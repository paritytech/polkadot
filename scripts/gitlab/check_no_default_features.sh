#!/usr/bin/env bash

set -e

pushd node/service && cargo check --no-default-features && popd
pushd cli && cargo check --no-default-features --features "service" && popd
