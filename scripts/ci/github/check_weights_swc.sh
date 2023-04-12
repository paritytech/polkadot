#!/usr/bin/env bash
# Need to set globstar for ** magic
shopt -s globstar

RUNTIME=$1
VERSION=$2
echo -e "$RUNTIME changes since $VERSION:\n"
echo '```'
swc compare commits \
    --method asymptotic \
    --offline \
    --path-pattern "./runtime/$RUNTIME/src/weights/**/*.rs" \
    "$VERSION"
    #--ignore-errors
echo '```'
