#!/usr/bin/env bash
set -e

if [ "$#" -ne 1 ]; then
	echo "Please provide the number of initial validators!"
	exit 1
fi

generate_account_id() {
	subkey ${3:-} inspect "$SECRET//$1//$2" | grep "Account ID" | awk '{ print $3 }'
}

generate_address() {
	subkey ${3:-} inspect "$SECRET//$1//$2" | grep "SS58 Address" | awk '{ print $3 }'
}

generate_address_and_account_id() {
	ACCOUNT=$(generate_account_id $1 $2 $3)
	ADDRESS=$(generate_address $1 $2 $3)
	if ${4:-false}; then
		INTO="unchecked_into"
	else
		INTO="into"
	fi

	printf "//$ADDRESS\nhex![\"${ACCOUNT#'0x'}\"].$INTO(),"
}

V_NUM=$1

AUTHORITIES=""

for i in $(seq 1 $V_NUM); do
	AUTHORITIES+="(\n"
	AUTHORITIES+="$(generate_address_and_account_id $i stash)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i controller)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i babe '--sr25519' true)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i grandpa '--ed25519' true)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i im_online '--sr25519' true)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i parachains '--sr25519' true)\n"
	AUTHORITIES+="$(generate_address_and_account_id $i authority_discovery '--sr25519' true)\n"
	AUTHORITIES+="),\n"
done

printf "$AUTHORITIES"
