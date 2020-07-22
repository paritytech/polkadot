#!/bin/sh

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

pull_companion_substrate

set -e

# WASM support is in progress. As more and more crates support WASM, we
# should add entries here. See https://github.com/paritytech/polkadot/issues/625
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path runtime/polkadot/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path runtime/kusama/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path erasure-coding/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path parachain/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path primitives/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path rpc/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path statement-table/Cargo.toml
time cargo build --locked --target=wasm32-unknown-unknown --manifest-path cli/Cargo.toml --no-default-features --features browser
