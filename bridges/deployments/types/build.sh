#!/bin/sh

# The script generates JSON type definition files in `./deployment` directory to be used for
# JS clients.
#
# It works by creating definitions for each side of the different bridge pairs we support
# (Rialto<>Millau and Rococo<>Wococo at the moment).
#
# To avoid duplication each bridge pair has a JSON file with common definitions, as well as a
# general JSON file with common definitions regardless of the bridge pair. These files are then
# merged with chain-specific type definitions.

set -eux

# Make sure we are in the right dir.
cd $(dirname $(realpath $0))

# Create types for our supported bridge pairs (Rialto<>Millau, Rococo<>Wococo)
jq -s '.[0] * .[1] * .[2]' rialto-millau.json common.json rialto.json > ../types-rialto.json
jq -s '.[0] * .[1] * .[2]' rialto-millau.json common.json millau.json > ../types-millau.json
jq -s '.[0] * .[1] * .[2]' rococo-wococo.json common.json rococo.json > ../types-rococo.json
jq -s '.[0] * .[1] * .[2]' rococo-wococo.json common.json wococo.json > ../types-wococo.json
