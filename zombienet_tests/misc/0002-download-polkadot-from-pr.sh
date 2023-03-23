#!/bin/bash

set -euxo pipefail

echo $@

CFG_DIR=/cfg

# add CFG_DIR as first `looking dir` to allow to overrides commands.
mkdir -p $CFG_DIR
export PATH=$CFG_DIR:$PATH

cd $CFG_DIR
# see 0002-upgrade-node.zndsl to view the args.
curl -L -O $1
chmod +x $CFG_DIR/polkadot
echo $(polkadot --version)
