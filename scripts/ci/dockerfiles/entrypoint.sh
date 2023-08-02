#!/usr/bin/env bash

# Sanity check
if [ -z "$BINARY" ]
then
    echo "BINARY ENV not defined, this should never be the case. Aborting..."
    exit 1
fi

echo "Starting binary $BINARY"
$BINARY $@
