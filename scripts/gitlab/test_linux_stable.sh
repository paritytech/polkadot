#!/usr/bin/env bash
set -e

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

time cargo t --features=disputes -p polkadot-node-core-dispute-coordinator

time cargo test --workspace --release --verbose --locked --features=runtime-benchmarks
