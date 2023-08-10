#!/usr/bin/env bash
set -e

# This script allows building a Container Image from a Linux
# binary that is injected into a base-image.

ENGINE=${ENGINE:-podman}

if [ "$ENGINE" == "podman" ]; then
  PODMAN_FLAGS="--format docker"
else
  PODMAN_FLAGS=""
fi

CONTEXT=$(mktemp -d)
REGISTRY=${REGISTRY:-docker.io}

# The following line ensure we know the project root
PROJECT_ROOT=${PROJECT_ROOT:-$(git rev-parse --show-toplevel)}
DOCKERFILE=${DOCKERFILE:-$PROJECT_ROOT/scripts/ci/dockerfiles/binary_injected.Dockerfile}
VERSION_TOML=$(grep "^version " $PROJECT_ROOT/Cargo.toml | grep -oE "([0-9\.]+-?[0-9]+)")

#n The following VAR have default that can be overriden
DOCKER_OWNER=${DOCKER_OWNER:-parity}

# We may get 1..n binaries, comma separated
BINARY=${BINARY:-polkadot}
IFS=',' read -r -a BINARIES <<< "$BINARY"

VERSION=${VERSION:-$VERSION_TOML}
BIN_FOLDER=${BIN_FOLDER:-.}

IMAGE=${IMAGE:-${REGISTRY}/${DOCKER_OWNER}/${BINARIES[0]}}
DESCRIPTION_DEFAULT="Injected Container image built for ${BINARY}"
DESCRIPTION=${DESCRIPTION:-$DESCRIPTION_DEFAULT}

VCS_REF=${VCS_REF:-01234567}

# Build the image
echo "Using engine: $ENGINE"
echo "Using Dockerfile: $DOCKERFILE"
echo "Using context: $CONTEXT"
echo "Building ${IMAGE}:latest container image for ${BINARY} v${VERSION} from ${BIN_FOLDER} hang on!"
echo "BIN_FOLDER=$BIN_FOLDER"
echo "CONTEXT=$CONTEXT"

# We need all binaries and resources available in the Container build "CONTEXT"
mkdir -p $CONTEXT/bin
for bin in "${BINARIES[@]}"
do
  echo "Copying $BIN_FOLDER/$bin to context: $CONTEXT/bin"
  cp "$BIN_FOLDER/$bin" "$CONTEXT/bin"
done

cp "$PROJECT_ROOT/scripts/ci/dockerfiles/entrypoint.sh" "$CONTEXT"

echo "Building image: ${IMAGE}"

TAGS=${TAGS[@]:-latest}
IFS=',' read -r -a TAG_ARRAY <<< "$TAGS"
TAG_ARGS=" "

echo "The image ${IMAGE} will be tagged with ${TAG_ARRAY[*]}"
for tag in "${TAG_ARRAY[@]}"; do
  TAG_ARGS+="--tag ${IMAGE}:${tag} "
done

echo "$TAG_ARGS"

# time \
$ENGINE build \
    ${PODMAN_FLAGS} \
    --build-arg VCS_REF="${VCS_REF}" \
    --build-arg BUILD_DATE=$(date -u '+%Y-%m-%dT%H:%M:%SZ') \
    --build-arg IMAGE_NAME="${IMAGE}" \
    --build-arg BINARY="${BINARY}" \
    --build-arg BIN_FOLDER="${BIN_FOLDER}" \
    --build-arg DESCRIPTION="${DESCRIPTION}" \
    ${TAG_ARGS} \
    -f "${DOCKERFILE}" \
    ${CONTEXT}

echo "Your Container image for ${IMAGE} is ready"
$ENGINE images

if [[ -z "${SKIP_IMAGE_VALIDATION}" ]]; then
  echo "Check the image ${IMAGE}:${TAG_ARRAY[0]}"
  $ENGINE run --rm -i "${IMAGE}:${TAG_ARRAY[0]}" --version

  echo "Query binaries"
  $ENGINE run --rm -i --entrypoint /bin/bash "${IMAGE}:${TAG_ARRAY[0]}" -c 'echo BINARY: $BINARY'
fi
