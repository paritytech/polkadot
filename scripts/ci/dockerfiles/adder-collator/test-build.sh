#!/usr/bin/env bash

TMP=$(mktemp -d)
ENGINE=${ENGINE:-podman}

# Fetch some binaries
$ENGINE run --user root --rm -it \
  -v "$TMP:/export" \
  --entrypoint /usr/bin/bash \
  paritypr/colander:master -c \
  'cp "$(which adder-collator)" /export'

$ENGINE run --user root --rm -it \
  -v "$TMP:/export" \
  --entrypoint /usr/bin/bash \
  paritypr/colander:master -c \
  'cp "$(which undying-collator)" /export'

./build-injected.sh $TMP
