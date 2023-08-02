# It would be better using this image to inject generic binaries
# instead of requiring a dockerfile per binary.
# Unfortunately, ENTRYPOINT requires using an ENV
# and the current setup shows limitation: the passed args are ignored.

FROM docker.io/parity/base-bin

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME
ARG BINARY=polkadot
ARG BIN_FOLDER=.
ARG DOC_URL=https://github.com/paritytech/polkadot
ARG DESCRIPTION="Polkadot: a platform for web3"

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="${DESCRIPTION}" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/binary_injected.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="${DOC_URL}"

USER root
WORKDIR /app

# add polkadot binary to docker image
COPY entrypoint.sh .
COPY ./$BIN_FOLDER/$BINARY /usr/local/bin/
RUN chmod a+rwx /usr/local/bin/$BINARY

USER parity
ENV BINARY=${BINARY}

# check if executable works in this container
RUN /usr/local/bin/$BINARY --version

VOLUME ["/$BINARY"]

# ENTRYPOINT
ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["--help"]
