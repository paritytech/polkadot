#!/bin/bash
#
# Run an instance of the Rococo -> Westend header sync.
#
# Right now this relies on local Westend and Rococo networks
# running (which include `pallet-bridge-grandpa` in their
# runtimes), but in the future it could use use public RPC nodes.

set -xeu

RUST_LOG=rpc=trace,bridge=trace ./target/debug/substrate-relay init-bridge RococoToWestend \
	--source-host 127.0.0.1 \
	--source-port 9955 \
	--target-host 127.0.0.1 \
	--target-port 9944 \
	--target-signer //Eve

RUST_LOG=rpc=trace,bridge=trace ./target/debug/substrate-relay relay-headers RococoToWestend \
	--source-host 127.0.0.1 \
	--source-port 9955 \
	--target-host 127.0.0.1 \
	--target-port 9944 \
	--target-signer //Bob \
	--prometheus-host=0.0.0.0 \
