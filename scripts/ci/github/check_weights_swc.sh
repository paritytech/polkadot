#!/usr/bin/env bash
# Need to set globstar for ** magic
shopt -s globstar

RUNTIME=$1
VERSION=$2
echo "<details>"
echo "<summary>Weight changes for $RUNTIME</summary>"
echo
swc compare commits \
    --method asymptotic \
    --offline \
    --path-pattern "./runtime/$RUNTIME/src/weights/**/*.rs" \
    --no-color \
    --format markdown \
    --strip-path-prefix "runtime/$RUNTIME/src/weights/" \
    "$VERSION"
    #--ignore-errors
echo
echo "</details>"
