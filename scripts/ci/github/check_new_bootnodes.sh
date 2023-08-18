#!/bin/bash
set -e
# shellcheck source=scripts/ci/common/lib.sh
source "$(dirname "${0}")/../common/lib.sh"

# This script checks any new bootnodes added since the last git commit

RUNTIMES=( kusama westend polkadot )

WAS_ERROR=0

for RUNTIME in "${RUNTIMES[@]}"; do
    CHAINSPEC_FILE="node/service/chain-specs/$RUNTIME.json"
    # Get the bootnodes from master's chainspec
    git show origin/master:"$CHAINSPEC_FILE" | jq '{"oldNodes": .bootNodes}' > "$RUNTIME-old-bootnodes.json"
    # Get the bootnodes from the current branch's chainspec
    git show HEAD:"$CHAINSPEC_FILE" | jq '{"newNodes": .bootNodes}' > "$RUNTIME-new-bootnodes.json"
    # Make a chainspec containing only the new bootnodes
    jq ".bootNodes = $(jq -rs '.[0] * .[1] | .newNodes-.oldNodes' \
        "$RUNTIME-new-bootnodes.json" "$RUNTIME-old-bootnodes.json")" \
        < "node/service/chain-specs/$RUNTIME.json" \
        > "$RUNTIME-new-chainspec.json"
    # exit early if the new chainspec has no bootnodes
    if [ "$(jq -r '.bootNodes | length' "$RUNTIME-new-chainspec.json")" -eq 0 ]; then
        echo "[+] No new bootnodes for $RUNTIME"
        # Clean up the temporary files
        rm "$RUNTIME-new-chainspec.json" "$RUNTIME-old-bootnodes.json" "$RUNTIME-new-bootnodes.json"
        continue
    fi
    # Check the new bootnodes
    if ! "scripts/ci/github/check_bootnodes.sh" "$RUNTIME-new-chainspec.json"; then
        WAS_ERROR=1
    fi
    # Clean up the temporary files
    rm "$RUNTIME-new-chainspec.json" "$RUNTIME-old-bootnodes.json" "$RUNTIME-new-bootnodes.json"
done


if [ $WAS_ERROR -eq 1 ]; then
    echo "[!] One of the new bootnodes failed to connect. Please check logs above."
    exit 1
fi
