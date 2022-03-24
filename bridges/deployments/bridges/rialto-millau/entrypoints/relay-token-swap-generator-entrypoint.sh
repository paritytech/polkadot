#!/bin/bash

# THIS SCRIPT IS NOT INTENDED FOR USE IN PRODUCTION ENVIRONMENT
#
# This scripts periodically calls the Substrate relay binary to generate messages. These messages
# are sent from the Millau network to the Rialto network.

set -eu

# Max delay before submitting transactions (s)
MAX_SUBMIT_DELAY_S=60
SOURCE_HOST=millau-node-charlie
SOURCE_PORT=9944
TARGET_HOST=rialto-node-charlie
TARGET_PORT=9944

# Sleep a bit between messages
rand_sleep() {
	SUBMIT_DELAY_S=`shuf -i 0-$MAX_SUBMIT_DELAY_S -n 1`
	echo "Sleeping $SUBMIT_DELAY_S seconds..."
	sleep $SUBMIT_DELAY_S
	NOW=`date "+%Y-%m-%d %H:%M:%S"`
	echo "Woke up at $NOW"
}

while true
do
	rand_sleep
	echo "Initiating token-swap between Rialto and Millau"
	/home/user/substrate-relay \
		swap-tokens \
		millau-to-rialto \
		--source-host $SOURCE_HOST \
		--source-port $SOURCE_PORT \
		--source-signer //WithRialtoTokenSwap \
		--source-balance 100000 \
		--target-host $TARGET_HOST \
		--target-port $TARGET_PORT \
		--target-signer //WithMillauTokenSwap \
		--target-balance 200000 \
		--target-to-source-conversion-rate-override metric \
		--source-to-target-conversion-rate-override metric \
		lock-until-block \
		--blocks-before-expire 32
done
