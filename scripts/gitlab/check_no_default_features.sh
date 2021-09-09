#!/usr/bin/env bash

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

time cargo check --no-default-features -p polkadot-service
time cargo check --no-default-features -p polkadot-cli
