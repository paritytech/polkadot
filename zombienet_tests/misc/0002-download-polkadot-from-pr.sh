#!/bin/bash

set -euxo pipefail

TEMP_DIR=/tmp/zombie

# add /tmp/zombie as first `looking dir` to allow to overrides commands.
mkdir -p $TEMP_DIR
export PATH=$TEMP_DIR:$PATH

cd $TEMP_DIR
# For testing using native provider you should set this env var
# POLKADOT_PR_BIN_URL=https://gitlab.parity.io/parity/mirrors/polkadot/-/jobs/1810914/artifacts/file/artifacts/polkadot
# with the version of polkadot you want to download.
curl -L -O $POLKADOT_PR_BIN_URL
chmod +x $TEMP_DIR/polkadot
echo $(polkadot --version)