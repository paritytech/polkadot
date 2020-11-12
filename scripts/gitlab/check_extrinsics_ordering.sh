#!/bin/bash
BIN=polkadot
LIVE_WS=wss://rpc.polkadot.io
LOCAL_WS=ws://localhost:9944

runtimes=(
  "westend"
  "kusama"
  "polkadot"
)

for RUNTIME in "${runtimes[@]}"; do
  # TODO: Write early exit if the transaction_version in runtime/$RUNTIME/src/lib.rs has
  # changed since the last release (use same technique as check_runtime.sh)
  # Start running the local polkadot node in the background
  $BIN --chain="$RUNTIME-local" > /dev/null 2>&1 &

  changed_extrinsics=$(
  # TODO: select websocket based on runtime
    polkadot-js-metadata-cmp "$LIVE_WS" "$LOCAL_WS" \
    | sed 's/^ \+//g' | grep -e 'idx: [0-9]\+ -> [0-9]\+'
  )

  if [ -n "$changed_extrinsics" ]; then
    echo "[!] Extrinsics indexing/ordering has changed! If this change is 
  intentional, please bump transaction_version in lib.rs. Changed extrinsics:"
    echo "$changed_extrinsics"
    exit 1
  fi
done

kill %1
