#!/usr/bin/env bash
set -e

# This script allows building a Container Image from a Linux
# binary that is injected into a base-image.

ENGINE=${ENGINE:-podman}

# The following line ensure we know the project root
PROJECT_ROOT=`git rev-parse --show-toplevel`
DOCKERFILE=${DOCKERFILE:-$PROJECT_ROOT/scripts/ci/dockerfiles/binary_injected.Dockerfile}
VERSION_TOML=$(grep "^version " $PROJECT_ROOT/Cargo.toml | grep -oE "([0-9\.]+-?[0-9]+)")

#n The following VAR have default that can be overriden
OWNER=${OWNER:-parity}
BINARY=${BINARY:-polkadot}
VERSION=${VERSION:-$VERSION_TOML}
BIN_FOLDER=${BIN_FOLDER:-.}

IMAGE=${OWNER}/${BINARY}
DESCRIPTION_DEFAULT="Injected Container image built for $BINARY"
DESCRIPTION=${DESCRIPTION:-$DESCRIPTION_DEFAULT}

# Build the image
echo "Using engine: $ENGINE"
echo "Using Dockerfile: $DOCKERFILE"
echo "Building ${IMAGE}:latest container image for ${BINARY} v${VERSION} from ${BIN_FOLDER} hang on!"

# time \
$ENGINE build \
    --build-arg BUILD_DATE=$(date -u '+%Y-%m-%dT%H:%M:%SZ') \
    --build-arg IMAGE_NAME=${IMAGE} \
    --build-arg BINARY=${BINARY} \
    --build-arg BIN_FOLDER=${BIN_FOLDER} \
    --build-arg DESCRIPTION=${DESCRIPTION} \
    -f $DOCKERFILE \
    -t ${IMAGE}:latest \
    -t ${IMAGE}:v${VERSION} \
    .

# Show the list of available images for this repo
echo "Your Container image for ${IMAGE} is ready"
$ENGINE images | grep ${IMAGE}

# Check the final image
$ENGINE run --rm -it "${IMAGE}:latest" --version
