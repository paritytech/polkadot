#!/bin/bash

### ARGS FOR THIS SCRIPT ###
# ./${SCRIPT_NAME} NAMESPACE LOG_PATH VALIDATOR_NUM
# All args have default values, specify args to override
# e.g: ./${SCRIPT_NAME} radu-test parity/substrate:some_feature /var/log/gurke 20

# =================================================================================================
#           Get cmdline arguments or assign default values to them.
# =================================================================================================

NAMESPACE=${1:-gurke-"$(random_string)"-runtest}
LOG_PATH=${3:-"${SCRIPT_PATH}/logs"}
VALIDATOR_NUM=${4:-20}

SCRIPT_NAME="$0"
SCRIPT_PATH=$(dirname "${SCRIPT_NAME}")     # relative
SCRIPT_PATH=$(cd "${SCRIPT_PATH}" && pwd)   # absolutized and normalized

# =================================================================================================
#           Determine if docker or podman is available.
# =================================================================================================

DOCKER_OR_PODMAN=$(which podman)
if [ $? -ne 0 ]; then
	DOCKER_OR_PODMAN=$(which docker)
	if [ $? -ne 0 ]; then
		echo "In order to run this script you need either podman or docker installed"
	fi
fi

echo "Running with $DOCKER_OR_PODMAN as container solution"


rm $SCRIPT_PATH/shared/seeds_file.txt

set -eou pipefail

# =================================================================================================
#          Generate a default chainspec and add validator keys to it
# =================================================================================================

SPEC_NAME=rococo-local.json
SHARED_DIR="$SCRIPT_PATH/shared"
mkdir -p $SHARED_DIR

echo "Generating a default local rococo chainspec"
$DOCKER_OR_PODMAN run -v -i --log-driver=none -a stdout parity/rococo:rococo-v1 build-spec --chain rococo-local --disable-default-bootnode | cat > $SHARED_DIR/$SPEC_NAME

echo "Removing default validators from spec"

$DOCKER_OR_PODMAN run -v $SHARED_DIR:/mnt/shared paritytech/simnetscripts:latest clear_authorities /mnt/shared/$SPEC_NAME
echo "Adding validators to the spec"

for VALIDATOR_IDX in $(seq 1 $VALIDATOR_NUM)
do
	VALIDATOR_SEED="Validator $VALIDATOR_IDX"
	echo $VALIDATOR_SEED
	echo $VALIDATOR_SEED >> $SCRIPT_PATH/shared/seeds_file.txt
	echo "//$VALIDATOR_SEED" > $SCRIPT_PATH/shared/$VALIDATOR_IDX.txt
done

echo "Adding validators"

$DOCKER_OR_PODMAN run -v $SHARED_DIR:/mnt/shared paritytech/simnetscripts:latest add_authorities_from_file /mnt/shared/$SPEC_NAME /mnt/shared/seeds_file.txt

# =================================================================================================
#         Run the testing
# =================================================================================================

function random_string {
   head -1 <(fold -w 30  <(tr -dc 'a-z0-9' < /dev/urandom))
}

mkdir -p "${SCRIPT_PATH}"/logs

echo "Running tests in namespace: ${NAMESPACE}"
echo "Storing scripts logs to: ${LOG_PATH}"

function forward_port {
  if is_port_forward_running ; then
    kill_previous_job
  fi
  start_forwading_job
}

FORWARD_GREP_FILTER='kubectl.*[p]ort-forward.*bootnode'

function is_port_forward_running {
  # shellcheck disable=SC2009
  ps aux | grep -qE "${FORWARD_GREP_FILTER}"
}

function kill_previous_job {
  # shellcheck disable=SC2009
  job_pid=$(ps aux | grep -E "${FORWARD_GREP_FILTER}" | awk '{ print $2 }')
  echo  "INFO Killed forwading port 9944 into bootnode"
  kill "${job_pid}"
}

function start_forwading_job {
  echo "INFO Started forwading port 9944 into bootnode"
  kubectl -n "${NAMESPACE}" \
          port-forward pod/bootnode 11222:9944 &> "${LOG_PATH}/forward-${NAMESPACE}.log" &
  sleep 2
}

set -x  # echo the commands to stdout
export SHARED_DIR=$SHARED_DIR
export NODE_COUNT=$VALIDATOR_NUM
/home/theodor/github.com/montekki/gurke/target/debug/gurke spawn --config "/home/theodor/github.com/montekki/gurke/examples/rococo/huge_local_rococo_testnet.toml" \
            -n "fedor-gurke"

forward_port

# Run tests
gurke test "${NAMESPACE}" "${SCRIPT_PATH}"/tests  --log-path "${LOG_PATH}"

