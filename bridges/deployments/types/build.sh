#!/bin/sh

# The script generates JSON type definition files in `./deployment` directory to be used for
# JS clients.
# Both networks have a lot of common types, so to avoid duplication we merge `common.json` file with
# chain-specific definitions in `rialto|millau.json`.

set -exu

# Make sure we are in the right dir.
cd $(dirname $(realpath $0))

# Create rialto and millau types.
jq -s '.[0] * .[1]' common.json rialto.json > ../types-rialto.json
jq -s '.[0] * .[1]' common.json millau.json > ../types-millau.json
