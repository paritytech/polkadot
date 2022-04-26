#!/usr/bin/env bash
set -e

pushd .

# The following line ensure we run from the project root
PROJECT_ROOT=`git rev-parse --show-toplevel`
cd $PROJECT_ROOT

# Find the current version from Cargo.toml
VERSION=`grep "^version" ./cli/Cargo.toml | egrep -o "([0-9\.]+-?[0-9]+)"`
GITUSER=parity
GITREPO=polkadot

# Build the image
echo "Building ${GITUSER}/${GITREPO}:latest docker image, hang on!"
time docker build \
    -f ./scripts/ci/dockerfiles/polkadot/polkadot_builder.Dockerfile \
    -t ${GITUSER}/${GITREPO}:latest \
    -t ${GITUSER}/${GITREPO}:v${VERSION} \
    .

# Show the list of available images for this repo
echo "Your Docker image for $GITUSER/$GITREPO is ready"
docker images | grep ${GITREPO}

popd
