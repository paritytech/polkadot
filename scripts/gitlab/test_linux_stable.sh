#!/usr/bin/env bash
set -e

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

time cargo test --release --locked -p polkadot-node-core-dispute-coordinator --features disputes
time cargo test --workspace --release --verbose --locked --features=runtime-benchmarks
