#!/bin/sh

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

pull_companion_substrate

set -e

# Check that the node will compile with `runtime-benchmarks` feature flag.
time cargo check --features runtime-benchmarks
