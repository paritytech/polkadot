#!/bin/bash

# Script used for running and updating bridge deployments.
#
# To deploy a network you can run this script with the name of the network you want to run.
#
# `./run.sh poa-rialto`
#
# To update a deployment to use the latest images available from the Docker Hub add the `update`
# argument after the bridge name.
#
# `./run.sh rialto-millau update`
#
# Once you've stopped having fun with your deployment you can take it down with:
#
# `./run.sh rialto-millau stop`

set -xeu

# Since the Compose commands are using relative paths we need to `cd` into the `deployments` folder.
cd "$( dirname "${BASH_SOURCE[0]}" )"

function show_help () {
  set +x
  echo " "
  echo Error: $1
  echo " "
  echo "Usage:"
  echo "  ./run.sh poa-rialto [stop|update]          Run PoA <> Rialto Networks & Bridge"
  echo "  ./run.sh rialto-millau [stop|update]       Run Rialto <> Millau Networks & Bridge"
  echo " "
  echo "Options:"
  echo "  --no-monitoring                            Disable monitoring"
  exit 1
}

RIALTO=' -f ./networks/rialto.yml'
MILLAU=' -f ./networks/millau.yml'
ETH_POA=' -f ./networks/eth-poa.yml'
MONITORING=' -f ./monitoring/docker-compose.yml'

BRIDGE=''
NETWORKS=''
SUB_COMMAND='start'
for i in "$@"
do
  case $i in
    --no-monitoring)
      MONITORING=" -f ./monitoring/disabled.yml"
      shift
      ;;
    poa-rialto)
      BRIDGE=$i
      NETWORKS+=${RIALTO}
      NETWORKS+=${ETH_POA}
      shift
      ;;
    rialto-millau)
      BRIDGE=$i
      NETWORKS+=${RIALTO}
      NETWORKS+=${MILLAU}
      shift
      ;;
    start|stop|update)
      SUB_COMMAND=$i
      shift
      ;;
    *)
      show_help "Unknown option: $i"
      ;;
  esac
done

if [ -z "$BRIDGE" ]; then
  show_help "Missing bridge name."
fi

BRIDGE_PATH="./bridges/$BRIDGE"
BRIDGE="-f $BRIDGE_PATH/docker-compose.yml"
COMPOSE_FILES=$BRIDGE$NETWORKS$MONITORING

# Compose looks for .env files in the the current directory by default, we don't want that
COMPOSE_ARGS="--project-directory . --env-file "
COMPOSE_ARGS+=$BRIDGE_PATH/.env

# Read and source variables from .env file so we can use them here
grep -e MATRIX_ACCESS_TOKEN -e WITH_PROXY $BRIDGE_PATH/.env > .env2 && . ./.env2 && rm .env2

if [ ! -z ${MATRIX_ACCESS_TOKEN+x} ]; then
  sed -i "s/access_token.*/access_token: \"$MATRIX_ACCESS_TOKEN\"/" ./monitoring/grafana-matrix/config.yml
fi

# Check the sub-command, perhaps we just mean to stop the network instead of starting it.
if [ "$SUB_COMMAND" == "stop" ]; then

  if [ ! -z ${WITH_PROXY+x} ]; then
    cd ./reverse-proxy
    docker-compose down
    cd -
  fi

  docker-compose $COMPOSE_ARGS $COMPOSE_FILES down

  exit 0
fi

# See if we want to update the docker images before starting the network.
if [ "$SUB_COMMAND" == "update" ]; then

  # Stop the proxy cause otherwise the network can't be stopped
  if [ ! -z ${WITH_PROXY+x} ]; then
    cd ./reverse-proxy
    docker-compose down
    cd -
  fi


  docker-compose $COMPOSE_ARGS $COMPOSE_FILES pull
  docker-compose $COMPOSE_ARGS $COMPOSE_FILES down
  docker-compose $COMPOSE_ARGS $COMPOSE_FILES build
fi

docker-compose $COMPOSE_ARGS $COMPOSE_FILES up -d

# Start the proxy if needed
if [ ! -z ${WITH_PROXY+x} ]; then
  cd ./reverse-proxy
  docker-compose up -d
fi
