#!/usr/bin/env bash

# Run a two node local net with adder-collator.

set -e

chainspec="rococo-local"

# disabled until we can actually successfully register the chain with polkadot-js-api
# if ! command -v polkadot-js-api > /dev/null; then
#   echo "polkadot-js-api required; try"
#   echo "  sudo yarn global add @polkadot/api-cli"
#   exit 1
# fi

PROJECT_ROOT=$(git rev-parse --show-toplevel)
# shellcheck disable=SC1090
source "$(dirname "$0")"/common.sh

cd "$PROJECT_ROOT"

last_modified_rust_file=$(
  find . -path ./target -prune -o -type f -name '*.rs' -printf '%T@ %p\n' |
  sort -nr |
  head -1 |
  cut -d' ' -f2-
)

polkadot="target/release/polkadot"
adder_collator="target/release/adder-collator"

# ensure the polkadot binary exists and is up to date
if [ ! -x "$polkadot" ] || [ "$polkadot" -ot "$last_modified_rust_file" ]; then
  cargo build --release
fi
# likewise for the adder collator
if [ ! -x "$adder_collator" ] || [ "$adder_collator" -ot "$last_modified_rust_file" ]; then
  cargo build --release -p test-parachain-adder-collator
fi

genesis="$(mktemp --directory)"
genesis_state="$genesis/state"
validation_code="$genesis/validation_code"

"$adder_collator" export-genesis-state > "$genesis_state"
"$adder_collator" export-genesis-wasm > "$validation_code"


# setup variables
node_offset=0
declare -a node_pids
declare -a node_pipes

# create a sed expression which injects the node name and stream type into each line
function make_sed_expr() {
  name="$1"
  type="$2"

  printf "s/^/%16s %s: /" "$name" "$type"
}

# turn a string into a flag
function flagify() {
  printf -- '--%s' "$(tr '[:upper:]' '[:lower:]' <<< "$1")"
}

# start a node and label its output
#
# This function takes a single argument, the node name.
# The name must be one of those which can be passed to the polkadot binary, in un-flagged form,
# one of:
#   alice, bob, charlie, dave, eve, ferdie, one, two
function run_node() {
  name="$1"
  # create a named pipe so we can get the node's PID while also sedding its output
  local stdout
  local stderr
  stdout=$(mktemp --dry-run --tmpdir)
  stderr=$(mktemp --dry-run --tmpdir)
  mkfifo "$stdout"
  mkfifo "$stderr"
  node_pipes+=("$stdout")
  node_pipes+=("$stderr")

  # compute ports from offset
  local port=$((30333+node_offset))
  local rpc_port=$((9933+node_offset))
  local ws_port=$((9944+node_offset))
  local prometheus_port=$((9615+node_offset))
  node_offset=$((node_offset+1))

  # start the node
  "$polkadot" \
    --chain "$chainspec" \
    --tmp \
    --port "$port" \
    --rpc-port "$rpc_port" \
    --ws-port "$ws_port" \
    --prometheus-port "$prometheus_port" \
    --rpc-cors all \
    "$(flagify "$name")" \
  > "$stdout" \
  2> "$stderr" \
  &
  local pid=$!
  node_pids+=("$pid")

  # send output from the stdout pipe to stdout, prepending the node name
  sed -e "$(make_sed_expr "$name" "OUT")" "$stdout" >&1 &
  # send output from the stderr pipe to stderr, prepending the node name
  sed -e "$(make_sed_expr "$name" "ERR")" "$stderr" >&2 &
}

# start an adder collator and label its output
#
# This function takes a single argument, the node name. This affects only the tagging.
function run_adder_collator() {
  name="$1"
  # create a named pipe so we can get the node's PID while also sedding its output
  local stdout
  local stderr
  stdout=$(mktemp --dry-run --tmpdir)
  stderr=$(mktemp --dry-run --tmpdir)
  mkfifo "$stdout"
  mkfifo "$stderr"
  node_pipes+=("$stdout")
  node_pipes+=("$stderr")

  # compute ports from offset
  local port=$((30333+node_offset))
  local rpc_port=$((9933+node_offset))
  local ws_port=$((9944+node_offset))
  local prometheus_port=$((9615+node_offset))
  node_offset=$((node_offset+1))

  # start the node
  "$adder_collator" \
    --chain "$chainspec" \
    --tmp \
    --port "$port" \
    --rpc-port "$rpc_port" \
    --ws-port "$ws_port" \
    --prometheus-port "$prometheus_port" \
    --rpc-cors all \
  > "$stdout" \
  2> "$stderr" \
  &
  local pid=$!
  node_pids+=("$pid")

  # send output from the stdout pipe to stdout, prepending the node name
  sed -e "$(make_sed_expr "$name" "OUT")" "$stdout" >&1 &
  # send output from the stderr pipe to stderr, prepending the node name
  sed -e "$(make_sed_expr "$name" "ERR")" "$stderr" >&2 &
}


# clean up the nodes when this script exits
function finish {
  for node_pid in "${node_pids[@]}"; do
    kill -9 "$node_pid"
  done
  for node_pipe in "${node_pipes[@]}"; do
    rm "$node_pipe"
  done
  rm -rf "$genesis"
}
trap finish EXIT

# start the nodes
run_node Alice
run_node Bob
run_adder_collator AdderCollator

# register the adder collator
# doesn't work yet due to https://github.com/polkadot-js/tools/issues/185
# polkadot-js-api \
#   --ws ws://localhost:9944 \
#   --sudo \
#   --seed "//Alice" \
#   tx.registrar.registerPara \
#     100 \
#     '{"scheduling":"Always"}' \
#     "@$validation_code" \
#     "@$genesis_state"

# now wait; this will exit on its own only if both subprocesses exit
# the practical implication, as both subprocesses are supposed to run forever, is that
# this script will also run forever, until killed, at which point the exit trap should kill
# the subprocesses
wait
