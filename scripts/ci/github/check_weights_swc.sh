#!/usr/bin/env bash
# Need to set globstar for ** magic
shopt -s globstar

RUNTIME=$1
VERSION=$2
swc compare commits \
    --method asymptotic \
    --offline \
    --path-pattern "./runtime/$RUNTIME/src/weights/**/*.rs" \
    --format csv \
    --no-color \
    "$VERSION"
