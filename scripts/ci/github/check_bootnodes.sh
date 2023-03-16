#!/usr/bin/env bash

# In this script, we check each bootnode for a given chainspec file and ensure they are contactable.
# We do this by removing every bootnode from the chainspec with the exception of the one
# we want to check. Then we spin up a node using this new chainspec, wait a little while
# and then check our local node's RPC endpoint for the number of peers. If the node hasn't
# been able to contact any other nodes, we can reason that the bootnode we used is not well-connected
# or is otherwise uncontactable.

# shellcheck source=scripts/ci/common/lib.sh
source "$(dirname "${0}")/../common/lib.sh"
CHAINSPEC_FILE="$1"
RUNTIME=$(basename "$CHAINSPEC_FILE" | cut -d '.' -f 1)

trap cleanup EXIT INT TERM

cleanup(){
    echo "[+] Script interrupted or ended. Cleaning up..."
    # Kill all the polkadot processes
    killall polkadot > /dev/null 2>&1
    exit $1
}

# count the number of bootnodes
BOOTNODES=$( jq -r '.bootNodes | length' "$CHAINSPEC_FILE" )
# Make a temporary dir for chainspec files
# Store an array of the bad bootnodes
BAD_BOOTNODES=()
GOOD_BOOTNODES=()
PIDS=()

echo "[+] Checking $BOOTNODES bootnodes for $RUNTIME"
for i in $(seq 0 $((BOOTNODES-1))); do
    BOOTNODE=$( jq -r .bootNodes["$i"] < "$CHAINSPEC_FILE" )
    # Check each bootnode in parallel
    check_bootnode "$BOOTNODE" "$CHAINSPEC_FILE" &
    PIDS+=($!)
    # Hold off 5 seconds between attempting to spawn nodes to stop the machine from getting overloaded
    sleep 5
done
RESPS=()
# Wait for all the nodes to finish
for pid in "${PIDS[@]}"; do
    wait "$pid"
    RESPS+=($?)
done
echo
# For any bootnodes that failed, add them to the bad bootnodes array
for i in "${!RESPS[@]}"; do
    if [ "${RESPS[$i]}" -ne 0 ]; then
        BAD_BOOTNODES+=("$( jq -r .bootNodes["$i"] < "$CHAINSPEC_FILE" )")
    fi
done
# For any bootnodes that succeeded, add them to the good bootnodes array
for i in "${!RESPS[@]}"; do
    if [ "${RESPS[$i]}" -eq 0 ]; then
        GOOD_BOOTNODES+=("$( jq -r .bootNodes["$i"] < "$CHAINSPEC_FILE" )")
    fi
done

# If we've got any uncontactable bootnodes for this runtime, print them
if [ ${#BAD_BOOTNODES[@]} -gt 0 ]; then
    echo "[!] Bad bootnodes found for $RUNTIME:"
    for i in "${BAD_BOOTNODES[@]}"; do
        echo "    $i"
    done
    cleanup 1
else
    echo "[+] All bootnodes for $RUNTIME are contactable"
    cleanup 0
fi
