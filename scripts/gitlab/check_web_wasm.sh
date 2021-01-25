#!/usr/bin/env bash

set -e

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

time cargo build --locked --target=wasm32-unknown-unknown --manifest-path cli/Cargo.toml --no-default-features --features browser
