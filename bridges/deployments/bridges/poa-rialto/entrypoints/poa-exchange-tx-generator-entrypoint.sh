#!/bin/bash

# THIS SCRIPT IS NOT INTENDED FOR USE IN PRODUCTION ENVIRONMENT
#
# This scripts periodically calls relay binary to generate PoA -> Substrate
# exchange transaction from hardcoded PoA senders (assuming they have
# enough funds) to hardcoded Substrate recipients.

set -eu

# Path to relay binary
RELAY_BINARY_PATH=${RELAY_BINARY_PATH:-./ethereum-poa-relay}
# Ethereum node host
ETH_HOST=${ETH_HOST:-poa-node-arthur}
# Ethereum node port
ETH_PORT=${ETH_PORT:-8545}
# Ethereum chain id
ETH_CHAIN_ID=${ETH_CHAIN_ID:-105}

# All possible Substrate recipients (hex-encoded public keys)
SUB_RECIPIENTS=(
	# Alice (5GrwvaEF...)
	"d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"\
	# Bob (5FHneW46...)
	"8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48"\
	# Charlie (5FLSigC9...)
	"90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22"\
	# Dave (5DAAnrj7...)
	"306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20"\
	# Eve (5HGjWAeF...)
	"e659a7a1628cdd93febc04a4e0646ea20e9f5f0ce097d9a05290d4a9e054df4e"\
	# Ferdie (5CiPPseX...)
	"1cbd2d43530a44705ad088af313e18f80b53ef16b36177cd4b77b846f2a5f07c"
)
# All possible Ethereum signers (hex-encoded private keys)
# (note that we're tracking nonce here => sender must not send concurrent transactions)
ETH_SIGNERS=(
	# Bertha account (0x007594304039c2937a12220338aab821d819f5a4) and its current nonce (unknown by default)
	"bc10e0f21e33456ade82182dd1ebdbdd89bca923d4e4adbd90fb5b44d7098cbe" ""
)
# Minimal exchange amount (in finney)
MIN_EXCHANGE_AMOUNT_FINNEY=${EXCHANGE_GEN_MIN_AMOUNT_FINNEY:-1} # 0.1 ETH
# Maximal exchange amount (in finney)
MAX_EXCHANGE_AMOUNT_FINNEY=${EXCHANGE_GEN_MAX_AMOUNT_FINNEY:-100000} # 100 ETH
# Max delay before submitting transactions (s)
MAX_SUBMIT_DELAY_S=${EXCHANGE_GEN_MAX_SUBMIT_DELAY_S:-60}

while true
do
	# sleep some time
	SUBMIT_DELAY_S=`shuf -i 0-$MAX_SUBMIT_DELAY_S -n 1`
	echo "Sleeping $SUBMIT_DELAY_S seconds..."
	sleep $SUBMIT_DELAY_S

	# select recipient
	SUB_RECIPIENTS_MAX_INDEX=$((${#SUB_RECIPIENTS[@]} - 1))
	SUB_RECIPIENT_INDEX=`shuf -i 0-$SUB_RECIPIENTS_MAX_INDEX -n 1`
	SUB_RECIPIENT=${SUB_RECIPIENTS[$SUB_RECIPIENT_INDEX]}

	# select signer
	ETH_SIGNERS_MAX_INDEX=$(((${#ETH_SIGNERS[@]} - 1) / 2))
	ETH_SIGNERS_INDEX=`shuf -i 0-$ETH_SIGNERS_MAX_INDEX -n 1`
	ETH_SIGNER_INDEX=$(($ETH_SIGNERS_INDEX * 2))
	ETH_SIGNER_NONCE_INDEX=$(($ETH_SIGNER_INDEX + 1))
	ETH_SIGNER=${ETH_SIGNERS[$ETH_SIGNER_INDEX]}
	ETH_SIGNER_NONCE=${ETH_SIGNERS[$ETH_SIGNER_NONCE_INDEX]}
	if [ -z $ETH_SIGNER_NONCE ]; then
		ETH_SIGNER_NONCE_ARG=
	else
		ETH_SIGNER_NONCE_ARG=`printf -- "--eth-nonce=%s" $ETH_SIGNER_NONCE`
	fi

	# select amount
	EXCHANGE_AMOUNT_FINNEY=`shuf -i $MIN_EXCHANGE_AMOUNT_FINNEY-$MAX_EXCHANGE_AMOUNT_FINNEY -n 1`
	EXCHANGE_AMOUNT_ETH=`printf "%s000" $EXCHANGE_AMOUNT_FINNEY`

	# submit transaction
	echo "Sending $EXCHANGE_AMOUNT_ETH from PoA:$ETH_SIGNER to Substrate:$SUB_RECIPIENT. Nonce: $ETH_SIGNER_NONCE"
	set -x
	SUBMIT_OUTPUT=`$RELAY_BINARY_PATH 2>&1 eth-submit-exchange-tx \
		--sub-recipient=$SUB_RECIPIENT \
		--eth-host=$ETH_HOST \
		--eth-port=$ETH_PORT \
		--eth-chain-id=$ETH_CHAIN_ID \
		--eth-signer=$ETH_SIGNER \
		--eth-amount=$EXCHANGE_AMOUNT_ETH \
		$ETH_SIGNER_NONCE_ARG`
	set +x

	# update sender nonce
	SUBMIT_OUTPUT_RE='nonce: ([0-9]+)'
	if [[ $SUBMIT_OUTPUT =~ $SUBMIT_OUTPUT_RE ]]; then
		ETH_SIGNER_NONCE=${BASH_REMATCH[1]}
		ETH_SIGNERS[$ETH_SIGNER_NONCE_INDEX]=$(($ETH_SIGNER_NONCE + 1))
	else
		echo "Missing nonce in relay response: $SUBMIT_OUTPUT"
		exit 1
	fi
done
