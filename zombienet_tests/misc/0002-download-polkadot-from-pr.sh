#!/bin/bash

set -euxo pipefail

TEMP_DIR=/tmp/zombie

# add /tmp/zombie as first `looking dir` to allow to overrides commands.
mkdir -p $TEMP_DIR
export PATH=$TEMP_DIR:$PATH

cd $TEMP_DIR
#curl -L -O https://gitlab.parity.io/parity/mirrors/polkadot/-/jobs/1810914/artifacts/file/artifacts/polkadot
curl -L -O $POLKADOT_PR_BIN_URL
chmod +x $TEMP_DIR/polkadot
echo $(polkadot --version)