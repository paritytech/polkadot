#!/usr/bin/env bash
# Need to set globstar for ** magic
shopt -s globstar

RUNTIME=$1
OLD=$2
NEW=$3
echo -e "$RUNTIME changes since $LAST_RELEASE:\n"
echo '```'
swc compare files \
    --old $(pwd)/$OLD/runtime/$RUNTIME/src/weights/**/*.rs \
    --new $(pwd)/$NEW/runtime/$RUNTIME/src/weights/**/*.rs \
    --method asymptotic \
    --ignore-errors
echo '```'
