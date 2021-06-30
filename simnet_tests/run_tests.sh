#!/bin/bash

### ARGS FOR THIS SCRIPT ###
# ./${SCRIPT_NAME} NAMESPACE IMAGE LOG_PATH
# All args have default values, specify args to override
# e.g: ./${SCRIPT_NAME} radu-test parity/substrate:some_feature /var/log/gurke 

set -eou pipefail
SCRIPT_NAME="$0"
SCRIPT_PATH=$(dirname "${SCRIPT_NAME}")     # relative
SCRIPT_PATH=$(cd "${SCRIPT_PATH}" && pwd)   # absolutized and normalized

function random_string {
   head -1 <(fold -w 30  <(tr -dc 'a-z0-9' < /dev/urandom))
 }

NAMESPACE=${1:-gurke-"$(random_string)"-runtest}
IMAGE=${2:-"docker.io/paritypr/synth-wave:master"}
LOG_PATH=${3:-"${SCRIPT_PATH}/logs"}
COLIMAGE=${4:-"docker.io/paritypr/colander:master"}
SCRIPTSIMAGE=${5:-"docker.io/paritytech/simnet:latest"}

mkdir -p "${SCRIPT_PATH}"/logs

echo "Running tests in namespace: ${NAMESPACE}"
echo "Testing image: ${IMAGE}"
echo "Storing scripts logs to: ${LOG_PATH}"
echo "Colator image is ${COLIMAGE}"
echo "SCRIPTSIMAGE image is ${SCRIPTSIMAGE}"


function forward_port {

  # RUN_IN_CONTAINER is env var that is set in the dockerfile
  # use the -v operator to explicitly test if a variable is set
  if  [[ ! -v RUN_IN_CONTAINER  ]]  ; then 
    if is_port_forward_running ; then
      kill_previous_job
    fi
  fi
  start_forwading_job 
}

FORWARD_GREP_FILTER='kubectl.*[p]ort-forward.*svc/rpc.*11222'

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
  kubectl -n "${NAMESPACE}" \
          expose pod bootnode \
          --name=rpc \
          --type=NodePort \
          --target-port=9944 \
          --port=9944
  kubectl -n "${NAMESPACE}" \
          port-forward svc/rpc 11222:9944 &> "${LOG_PATH}/forward-${NAMESPACE}.log" &
  sleep 2
  echo "INFO Started forwading port 9944 into bootnode"
}

function update_api {
  pwd
  cd "${SCRIPT_PATH}"/../../simnet_scripts/
  npm run build
  cd -
  pwd
}

echo "INFO: Checking if namespace has no pods"
kubectl -n "${NAMESPACE}" get pods

export NAMESPACE="${NAMESPACE}"
export COLIMAGE="${COLIMAGE}"
export SYNTHIMAGE="${IMAGE}"
export SCRIPTSIMAGE="${SCRIPTSIMAGE}"

cd "${SCRIPT_PATH}"

set -x  # echo the commands to stdout
gurke  spawn --config "${SCRIPT_PATH}"/configs/simple_rococo_testnet.toml \
            -n "${NAMESPACE}" \
            --image "${IMAGE}"

echo "INFO: Checking if pods launched correctly"
kubectl -n "${NAMESPACE}" get pods -o wide
echo "INFO: Updating Polkadot JS API"
update_api
forward_port

# Run tests
gurke test "${NAMESPACE}" "${SCRIPT_PATH}"/tests  --log-path "${LOG_PATH}"

