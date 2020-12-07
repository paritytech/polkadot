#!/usr/bin/env bash
set -e

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

time cargo test --all --release --verbose --locked --features=runtime-benchmarks --features=real-overseer

cd parachain/test-parachains/adder/collator/
time cargo test --release --verbose --locked --features=real-overseer
