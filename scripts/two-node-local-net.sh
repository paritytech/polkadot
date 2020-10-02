#!/usr/bin/env bash

# run a two node local net

set -e

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

# ensure the polkadot binary exists and is up to date
if [ ! -x "$polkadot" ] || [ "$polkadot" -ot "$last_modified_rust_file" ]; then
  cargo build --release
fi

# setup variables
node_offset=0
declare -a node_pids
declare -a node_pipes

# create a sed expression which injects the node name and stream type into each line
function make_sed_expr() {
  name="$1"
  type="$2"

  printf "s/^/%8s %s: /" "$name" "$type"
}

# start a node and name its output
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

  # compute offsets
  local port=$((30333+node_offset))
  local rpc_port=$((9933+node_offset))
  local ws_port=$((9944+node_offset))
  node_offset++

  # start the node
  # TODO: do we need a custom chainspec? Probably.
  "$polkadot" \
    --chain polkadot \
    --tmp \
    --dev \
    --port "$port" \
    --rpc-port "$rpc_port" \
    --ws-port "$ws_port" \
    --rpc-external \
    --ws-external \
    --rpc-cors all \
    --validator \
    --name "$name" \
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
}
trap finish EXIT

# start the nodes
run_node alice
run_node bob

# now wait; this will exit on its own only if both subprocesses exit
# the practical implication, as both subprocesses are supposed to run forever, is that
# this script will also run forever, until killed, at which point the exit trap should kill
# the subprocesses
wait
