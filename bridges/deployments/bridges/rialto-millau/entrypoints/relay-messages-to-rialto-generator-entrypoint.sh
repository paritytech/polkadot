#!/bin/bash

# THIS SCRIPT IS NOT INTENDED FOR USE IN PRODUCTION ENVIRONMENT
#
# This scripts periodically calls the Substrate relay binary to generate messages. These messages
# are sent from the Millau network to the Rialto network.

set -eu

# Max delay before submitting transactions (s)
MAX_SUBMIT_DELAY_S=${MSG_EXCHANGE_GEN_MAX_SUBMIT_DELAY_S:-30}
MESSAGE_LANE=${MSG_EXCHANGE_GEN_LANE:-00000000}
SECONDARY_MESSAGE_LANE=${MSG_EXCHANGE_GEN_SECONDARY_LANE}
MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE=128
FERDIE_ADDR=6ztG3jPnJTwgZnnYsgCDXbbQVR82M96hBZtPvkN56A9668ZC

SHARED_CMD=" /home/user/substrate-relay send-message MillauToRialto"
SHARED_HOST="--source-host millau-node-bob --source-port 9944"
DAVE_SIGNER="--target-signer //Dave --source-signer //Dave"

SEND_MESSAGE="$SHARED_CMD $SHARED_HOST $DAVE_SIGNER"

# Sleep a bit between messages
rand_sleep() {
	SUBMIT_DELAY_S=`shuf -i 0-$MAX_SUBMIT_DELAY_S -n 1`
	echo "Sleeping $SUBMIT_DELAY_S seconds..."
	sleep $SUBMIT_DELAY_S
}

# start sending large messages immediately
LARGE_MESSAGES_TIME=0
# start sending message packs in a hour
BUNCH_OF_MESSAGES_TIME=3600

while true
do
	rand_sleep
	echo "Sending Remark from Millau to Rialto using Target Origin"
	$SEND_MESSAGE \
		--lane $MESSAGE_LANE \
		--origin Target \
		remark

	if [ ! -z $SECONDARY_MESSAGE_LANE ]; then
		echo "Sending Remark from Millau to Rialto using Target Origin using secondary lane: $SECONDARY_MESSAGE_LANE"
		$SEND_MESSAGE \
			--lane $SECONDARY_MESSAGE_LANE \
			--origin Target \
			remark
	fi

	rand_sleep
	echo "Sending Transfer from Millau to Rialto using Target Origin"
	 $SEND_MESSAGE \
		--lane $MESSAGE_LANE \
		--origin Target \
		transfer \
		--amount 1000000000 \
		--recipient $FERDIE_ADDR

	rand_sleep
	echo "Sending Remark from Millau to Rialto using Source Origin"
	 $SEND_MESSAGE \
		--lane $MESSAGE_LANE \
		--origin Source \
		remark

	rand_sleep
	echo "Sending Transfer from Millau to Rialto using Source Origin"
	 $SEND_MESSAGE \
		--lane $MESSAGE_LANE \
		--origin Source \
		transfer \
		--amount 1000000000 \
		--recipient $FERDIE_ADDR

	# every other hour we're sending 3 large (size, weight, size+weight) messages
	if [ $SECONDS -ge $LARGE_MESSAGES_TIME ]; then
		LARGE_MESSAGES_TIME=$((SECONDS + 7200))

		rand_sleep
		echo "Sending Maximal Size Remark from Millau to Rialto using Target Origin"
		$SEND_MESSAGE \
			--lane $MESSAGE_LANE \
			--origin Target \
			remark \
			--remark-size=max

		rand_sleep
		echo "Sending Maximal Dispatch Weight Remark from Millau to Rialto using Target Origin"
		$SEND_MESSAGE \
			--lane $MESSAGE_LANE \
			--origin Target \
			--dispatch-weight=max \
			remark

		rand_sleep
		echo "Sending Maximal Size and Dispatch Weight Remark from Millau to Rialto using Target Origin"
		$SEND_MESSAGE \
			--lane $MESSAGE_LANE \
			--origin Target \
			--dispatch-weight=max \
			remark \
			--remark-size=max

	fi

	# every other hour we're sending a bunch of small messages
	if [ $SECONDS -ge $BUNCH_OF_MESSAGES_TIME ]; then
		BUNCH_OF_MESSAGES_TIME=$((SECONDS + 7200))

		for i in $(seq 1 $MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE);
		do
			$SEND_MESSAGE \
				--lane $MESSAGE_LANE \
				--origin Target \
				remark
		done

	fi
done
