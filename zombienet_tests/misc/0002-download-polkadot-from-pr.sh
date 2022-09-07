#!/bin/bash

set -euxo pipefail

TEMP_DIR=/tmp/zombie

# add /tmp/zombie as first `looking dir` to allow to overrides commands.
mkdir -p $TEMP_DIR
export PATH=$TEMP_DIR:$PATH

cd $TEMP_DIR
# see 0002-upgrade-node.feature to view the args.
curl -L -O $1
chmod +x $TEMP_DIR/polkadot
echo $(polkadot --version)