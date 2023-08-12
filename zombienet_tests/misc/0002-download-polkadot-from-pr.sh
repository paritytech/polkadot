#!/bin/bash

set -euxo pipefail

echo $@

CFG_DIR=/cfg

# add CFG_DIR as first `looking dir` to allow to overrides commands.
mkdir -p $CFG_DIR
export PATH=$CFG_DIR:$PATH

cd $CFG_DIR
# see 0002-upgrade-node.zndsl to view the args.
curl -L -O $1/polkadot
curl -L -O $1/polkadot-prepare-worker
curl -L -O $1/polkadot-execute-worker
chmod +x $CFG_DIR/polkadot $CFG_DIR/polkadot-prepare-worker $CFG_DIR/polkadot-execute-worker
echo $(polkadot --version)
