#!/bin/sh

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

pull_companion_substrate

# Test Polkadot pr or master branch with this Substrate commit.
time cargo test --all --release --verbose --locked --features runtime-benchmarks
