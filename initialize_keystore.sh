#!/bin/bash

helpFunction()
{
   echo ""
   echo "Usage: $0 <chain_spec.json> <base-path> <node-key-file>"
   echo -e "\t chain_spec.json: Path to the chain specification"
   echo -e "\t base-path: Base path to be used for the node"
   echo -e "\t node-key-file Path to the node-key-file"
   echo ""
   exit 1 # Exit script after printing help
}

# Allow for '-v' to be used for checking the script version
while getopts "v" option; do
   case $option in
      v) echo v1.0.0 && exit;;
   esac
done

# Print helpFunction in case parameters are empty
if [ -z "$1" ] || [ -z "$2" ] || [ -z "$3" ]
then
   echo "Unexpected number of arguments, empty arguments are not valid";
   helpFunction
fi

chainSpec="$1"
basePath="$2"
nodeKeyFile="$3"

SEED=$(cat $nodeKeyFile);
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme ed25519 --suri "0x${SEED}" --key-type gran;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme sr25519 --suri "0x${SEED}" --key-type babe;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme sr25519 --suri "0x${SEED}" --key-type imon;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme sr25519 --suri "0x${SEED}" --key-type para;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme sr25519 --suri "0x${SEED}" --key-type asgn;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme sr25519 --suri "0x${SEED}" --key-type audi;
./polkadot key insert --base-path $basePath --chain $chainSpec --scheme ecdsa --suri "0x${SEED}" --key-type beef;