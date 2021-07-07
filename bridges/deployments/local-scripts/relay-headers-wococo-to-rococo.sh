#!/bin/bash
#
# Run an instance of the Wococo -> Rococo header sync.
#
# Right now this relies on local Wococo and Rococo networks
# running (which include `pallet-bridge-grandpa` in their
# runtimes), but in the future it could use use public RPC nodes.

set -xeu

RUST_LOG=rpc=trace,bridge=trace ./target/debug/substrate-relay init-bridge WococoToRococo \
	--source-host 127.0.0.1 \
	--source-port 9944 \
	--target-host 127.0.0.1 \
	--target-port 9955 \
	--target-signer //Alice

RUST_LOG=rpc=trace,bridge=trace ./target/debug/substrate-relay relay-headers WococoToRococo \
	--source-host 127.0.0.1 \
	--source-port 9944 \
	--target-host 127.0.0.1 \
	--target-port 9955 \
	--target-signer //Charlie \
	--prometheus-host=0.0.0.0 \
