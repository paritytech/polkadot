#!/usr/bin/env bash

# In this script, we check each bootnode for each runtime and ensure they are contactable.
# We do this by removing every bootnode from the chainspec with the exception of the one
# we want to check. Then we spin up a node using this new chainspec, wait a little while
# and then check our local node's RPC endpoint for the number of peers. If the node hasn't
# been able to contact any other nodes, we can reason that the bootnode we used is not well-connected
# or is otherwise uncontactable.

# Root of the polkadot dir
ROOT="$(dirname "${0}")/../../.."
RUNTIME="$1"

trap cleanup EXIT INT TERM

cleanup(){
    echo "[+] Script interrupted or ended. Cleaning up..."
    # Kill all the polkadot processes
    killall polkadot > /dev/null 2>&1
}

check_bootnode(){
    BOOTNODE_INDEX=$1
    TMP_CHAINSPEC_FILE="$RUNTIME.$BOOTNODE_INDEX.tmp.json"
    FINAL_CHAINSPEC_FILE="$RUNTIME.$BOOTNODE_INDEX.final.json"
    # Copy the chainspec file to a temporary location to avoid weird race conditions when running in parallel
    cp "$CHAINSPEC_FILE" "$CHAINSPEC_TMPDIR/$TMP_CHAINSPEC_FILE"
    pushd "$CHAINSPEC_TMPDIR" > /dev/null || exit 1
        jq ".bootNodes |= [.[$BOOTNODE_INDEX]] " < "$TMP_CHAINSPEC_FILE" > "$FINAL_CHAINSPEC_FILE"
        BOOTNODE=$( jq -r '.bootNodes[0]' < "$FINAL_CHAINSPEC_FILE" )
        # Get the first ephemeral port
        if [[ "$OSTYPE" == "darwin"* ]]; then
            BASE_PORT=$(sysctl net.inet.ip.portrange.first | awk '{print $2}')
        else
            BASE_PORT=$(sysctl net.ipv4.ip_local_port_range | awk '{print $3}')
        fi
        # If we're on a mac, use this instead
        # BASE_PORT=$(sysctl net.inet.ip.portrange.first | awk '{print $2}')
        RPC_PORT=$((BASE_PORT + BOOTNODE_INDEX))
        echo "[+] Checking bootnode $BOOTNODE"
        polkadot --chain "$FINAL_CHAINSPEC_FILE" --no-mdns --rpc-port=$RPC_PORT --tmp  > /dev/null 2>&1 &
        POLKADOT_PID=$!
        # We just spun up a bunch of nodes... probably want to wait a bit.
        sleep 60
    popd > /dev/null || exit 1
    # Check the health endpoint of the RPC node
    PEERS="$(curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"system_health","params":[],"id":1}' http://localhost:$RPC_PORT | jq -r '.result.peers')"
    # Clean up the node
    kill -9 $POLKADOT_PID
    # Sometimes due to machine load or other reasons, we don't get a response from the RPC node
    # If $PEERS is an empty variable, mark the node as unreachable
    if [ -z "$PEERS" ]; then
        PEERS=0
    fi
    if [ "$PEERS" -gt 0 ]; then
        echo "[+] $PEERS peers found for $BOOTNODE"
        echo "    Bootnode appears contactable"
        GOOD_BOOTNODES+=("$BOOTNODE")
        return 0
    else
        echo "[!] $PEERS peers found for $BOOTNODE"
        echo "    Bootnode appears unreachable"
        BAD_BOOTNODES+=("$BOOTNODE")
        return 1
    fi
}

# For each runtime
CHAINSPEC_FILE="$ROOT/node/service/chain-specs/$RUNTIME.json"
# count the number of bootnodes
BOOTNODES=$( jq -r '.bootNodes | length' "$CHAINSPEC_FILE" )
# Make a temporary dir for chainspec files
CHAINSPEC_TMPDIR="$(mktemp -d -t "${RUNTIME}_chainspecs_XXXXXX")"
echo "[+] Using $CHAINSPEC_TMPDIR as temporary chainspec dir"
# Store an array of the bad bootnodes
BAD_BOOTNODES=()
GOOD_BOOTNODES=()
PIDS=()
echo "[+] Checking $BOOTNODES bootnodes for $RUNTIME"
for i in $(seq 0 $((BOOTNODES-1))); do
    # Check each bootnode in parallel
    check_bootnode "$i" &
    PIDS+=($!)
    # Hold off one second between attempting to spawn nodes
    sleep 1
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
    cleanup
    exit 1
else
    echo "[+] All bootnodes for $RUNTIME are contactable"
    cleanup
    exit 0
fi