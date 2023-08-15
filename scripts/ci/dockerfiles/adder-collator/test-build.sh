#!/usr/bin/env bash

TMP=$(mktemp -d)
ENGINE=${ENGINE:-podman}

# TODO: Switch to /bin/bash when the image is built from parity/base-bin

# Fetch some binaries
$ENGINE run --user root --rm -i \
  --pull always \
  -v "$TMP:/export" \
  --entrypoint /usr/bin/bash \
  paritypr/colander:master -c \
  'cp "$(which adder-collator)" /export'

$ENGINE run --user root --rm -i \
  --pull always \
  -v "$TMP:/export" \
  --entrypoint /usr/bin/bash \
  paritypr/colander:master -c \
  'cp "$(which undying-collator)" /export'

./build-injected.sh $TMP
