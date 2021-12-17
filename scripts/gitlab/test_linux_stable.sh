#!/usr/bin/env bash
set -eux

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

# Builds with the runtime benchmarks/metrics features are only to be used for testing.
time cargo test --workspace --release --verbose --locked --features=runtime-benchmarks,runtime-metrics
