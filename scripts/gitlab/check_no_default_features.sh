#!/usr/bin/env bash

set -e

time cargo check --no-default-features -p polkadot-service
time cargo check --no-default-features -p polkadot-cli
