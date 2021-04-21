#!/bin/bash

# Used for manually sending a message to a running network.
#
# You could for example spin up a full network using the Docker Compose files
# we have (to make sure the message relays are running), but remove the message
# generator service. From there you may submit messages manually using this script.

RIALTO_PORT="${RIALTO_PORT:-9944}"

case "$1" in
	remark)
		RUST_LOG=runtime=trace,substrate-relay=trace,bridge=trace \
		./target/debug/substrate-relay send-message RialtoToMillau \
			--source-host localhost \
			--source-port $RIALTO_PORT \
			--target-signer //Alice \
			--source-signer //Bob \
			--lane 00000000 \
			--origin Target \
			remark \
		;;
	transfer)
		RUST_LOG=runtime=trace,substrate-relay=trace,bridge=trace \
		./target/debug/substrate-relay send-message RialtoToMillau \
			--source-host localhost \
			--source-port $RIALTO_PORT \
			--target-signer //Alice \
			--source-signer //Bob \
			--lane 00000000 \
			--origin Target \
			transfer \
			--amount 100000000000000 \
			--recipient 5DZvVvd1udr61vL7Xks17TFQ4fi9NiagYLaBobnbPCP14ewA \
		;;
	*) echo "A message type is require. Supported messages: remark, transfer."; exit 1;;
esac
