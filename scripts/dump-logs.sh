#!/bin/bash

# A script to dump logs from selected important docker containers
# to make it easier to analyze locally.

set -xeu

DATE=$(date +"%Y-%m-%d-%T")
LOGS_DIR="${DATE//:/-}-logs"
mkdir $LOGS_DIR
cd $LOGS_DIR

# From $ docker ps --format '{{.Names}}'

SERVICES=(\
	deployments_relay-messages-millau-to-rialto-generator_1 \
	deployments_relay-messages-rialto-to-millau-generator_1 \
	deployments_relay-messages-millau-to-rialto-lane-00000001_1 \
	deployments_relay-messages-rialto-to-millau-lane-00000001_1 \
	deployments_relay-millau-rialto_1 \
	deployments_relay-headers-westend-to-millau_1 \
	deployments_rialto-node-alice_1 \
	deployments_rialto-node-bob_1 \
	deployments_millau-node-alice_1 \
	deployments_millau-node-bob_1 \
)

for SVC in ${SERVICES[*]}
do
	SHORT_NAME="${SVC//deployments_/}"
	docker logs $SVC &> $SHORT_NAME.log | true
done

cd -
tar cvjf $LOGS_DIR.tar.bz2 $LOGS_DIR
