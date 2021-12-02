#!/bin/bash
set -xeu

/home/user/rialto-bridge-node build-spec \
	--chain local \
	--raw \
	--disable-default-bootnode \
	> /rialto-share/rialto-relaychain-spec-raw.json

# we're using local driver + tmpfs for shared `/rialto-share` volume, which is populated
# by the container running this script. If this script ends, the volume will be detached
# and our chain spec will be lost when it'll go online again. Hence the never-ending
# script which keeps volume online until container is stopped.
tail -f /dev/null
