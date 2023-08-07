#!/usr/bin/env bash

TMP=$(mktemp -d)
ENGINE=${ENGINE:-podman}

# You need to build an injected image first

# Fetch some binaries
$ENGINE run --user root --rm -it \
  -v "$TMP:/export" \
  --entrypoint /bin/bash \
  parity/staking-miner -c \
  'cp "$(which staking-miner)" /export'

echo "Checking TMP:"
tree $TMP

./build-injected.sh $TMP
