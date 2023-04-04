#!/bin/bash

RUNTIME=$1
# Get last release from github
RELEASE=$(curl -s https://api.github.com/repos/paritytech/polkadot/releases/latest | jq -r .tag_name)

swc compare commits --method asymptotic --path-pattern "./runtime/$RUNTIME/src/weights/**/*.rs" "$RELEASE" | tee swc_output_"$RUNTIME".txt
