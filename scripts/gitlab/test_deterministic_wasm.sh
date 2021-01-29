#!/usr/bin/env bash

set -e

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

# build runtime
WASM_BUILD_NO_COLOR=1 cargo build --verbose --release -p runtime-kusama -p runtime-polkadot -p runtime-westend
# make checksum
sha256sum target/release/wbuild/runtime-*/target/wasm32-unknown-unknown/release/*.wasm > checksum.sha256
# clean up - FIXME: can we reuse some of the artifacts?
cargo clean
# build again
WASM_BUILD_NO_COLOR=1 cargo build --verbose --release -p runtime-kusama -p runtime-polkadot -p runtime-westend
# confirm checksum
sha256sum -c checksum.sha256
