#!/usr/bin/env bash
set -e

# Include the common functions library
#shellcheck source=../common/lib.sh
. "$(dirname "${0}")/../common/lib.sh"

HEAD_BIN=./artifacts/polkadot
HEAD_WS=ws://localhost:9944
RELEASE_WS=ws://localhost:9945

runtimes=(
  "westend"
  "kusama"
  "polkadot"
)

# First we fetch the latest released binary
latest_release=$(latest_release 'paritytech/polkadot')
RELEASE_BIN="./polkadot-$latest_release"
echo "[+] Fetching binary for Polkadot version $latest_release"
curl -L "https://github.com/paritytech/polkadot/releases/download/$latest_release/polkadot" > "$RELEASE_BIN" || exit 1
chmod +x "$RELEASE_BIN"


for RUNTIME in "${runtimes[@]}"; do
  echo "[+] Checking runtime: ${RUNTIME}"

  release_transaction_version=$(
    git show "origin/release:runtime/${RUNTIME}/src/lib.rs" | \
      grep 'transaction_version'
  )

  current_transaction_version=$(
    grep 'transaction_version' "./runtime/${RUNTIME}/src/lib.rs"
  )

  echo "[+] Release: ${release_transaction_version}"
  echo "[+] Ours: ${current_transaction_version}"

  if [ ! "$release_transaction_version" = "$current_transaction_version" ]; then
    echo "[+] Transaction version for ${RUNTIME} has been bumped since last release."
    exit 0
  fi

  # Start running the nodes in the background
  $HEAD_BIN --chain="$RUNTIME-local" --tmp &
  $RELEASE_BIN --chain="$RUNTIME-local" --ws-port 9945 --tmp &
  jobs

  # Sleep a little to allow the nodes to spin up and start listening
  TIMEOUT=5
  for i in $(seq $TIMEOUT); do
    sleep 1
      if [ "$(lsof -nP -iTCP -sTCP:LISTEN | grep -c '994[45]')" == 2 ]; then
        echo "[+] Both nodes listening"
        break
      fi
      if [ "$i" == $TIMEOUT ]; then
        echo "[!] Both nodes not listening after $i seconds. Exiting"
        exit 1
      fi
  done
  sleep 5

  changed_extrinsics=$(
    polkadot-js-metadata-cmp "$RELEASE_WS" "$HEAD_WS" \
      | sed 's/^ \+//g' | grep -e 'idx: [0-9]\+ -> [0-9]\+' || true
  )

  if [ -n "$changed_extrinsics" ]; then
    echo "[!] Extrinsics indexing/ordering has changed in the ${RUNTIME} runtime! If this change is intentional, please bump transaction_version in lib.rs. Changed extrinsics:"
    echo "$changed_extrinsics"
    exit 1
  fi

  echo "[+] No change in extrinsics ordering for the ${RUNTIME} runtime"
  jobs -p | xargs kill; sleep 5
done

# Sleep a little to let the jobs die properly
sleep 5
