#!/usr/bin/env bash

TMP=$(mktemp -d)
ENGINE=${ENGINE:-podman}

# Fetch some binaries
$ENGINE run --user root --rm -it \
  --pull always \
  -v "$TMP:/export" \
  --entrypoint /bin/bash \
  paritypr/malus:7217 -c \
  'cp "$(which malus)" /export'

echo "Checking binaries we got:"
ls -al $TMP

./build-injected.sh $TMP
