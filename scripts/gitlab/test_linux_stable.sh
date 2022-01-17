#!/usr/bin/env bash
set -eux

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

# Builds with the runtime benchmarks/metrics features are only to be used for testing.
time cargo test --workspace --profile testnet --verbose --locked --features=runtime-benchmarks,runtime-metrics

# We need to separately run the `polkadot-node-metrics` tests. More specifically, because the 
# `runtime_can_publish_metrics` test uses the `test-runtime` which doesn't support
# the `runtime-benchmarks` feature.
time cargo test  --profile testnet --verbose --locked --features=runtime-metrics -p polkadot-node-metrics
