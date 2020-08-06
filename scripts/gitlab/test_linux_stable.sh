#!/usr/bin/env bash

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

skip_if_companion_pr

time cargo test --all --release --verbose --locked --features runtime-benchmarks
