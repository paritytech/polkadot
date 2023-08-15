FROM docker.io/parity/base-bin

# This file allows building a Generic container image
# based on one or multiple pre-built Linux binaries.
# Some defaults are set to polkadot but all can be overriden.

SHELL ["/bin/bash", "-c"]

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME

# That can be a single one or a comma separated list
ARG BINARY=polkadot

ARG BIN_FOLDER=.
ARG DOC_URL=https://github.com/paritytech/polkadot
ARG DESCRIPTION="Polkadot: a platform for web3"
ARG AUTHORS="devops-team@parity.io"
ARG VENDOR="Parity Technologies"

LABEL io.parity.image.authors=${AUTHORS} \
	io.parity.image.vendor="${VENDOR}" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="${DOC_URL}" \
	io.parity.image.description="${DESCRIPTION}" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/binary_injected.Dockerfile"

USER root
WORKDIR /app

# add polkadot binary to docker image
# sample for polkadot: COPY ./polkadot ./polkadot-*-worker /usr/local/bin/
COPY entrypoint.sh .
COPY "bin/*" "/usr/local/bin/"
RUN chmod -R a+rx "/usr/local/bin"

USER parity
ENV BINARY=${BINARY}

# ENTRYPOINT
ENTRYPOINT ["/app/entrypoint.sh"]

# We call the help by default
CMD ["--help"]
