#!/usr/bin/env bash

# Sanity check
if [ -z "$BINARY" ]
then
    echo "BINARY ENV not defined, this should never be the case. Aborting..."
    exit 1
fi

# If the user built the image with multiple binaries,
# we consider the first one to be the canonical one
# To start with another binary, the user can either:
#  - use the --entrypoint option
#  - pass the ENV BINARY with a single binary
IFS=',' read -r -a BINARIES <<< "$BINARY"
BIN0=${BINARIES[0]}
echo "Starting binary $BIN0"
$BIN0 $@
