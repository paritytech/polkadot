FROM docker.io/parity/base-bin

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME
ARG DOC_URL=https://github.com/paritytech/polkadot
ARG DESCRIPTION="Polkadot: a platform for web3"

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="${DESCRIPTION}" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/polkadot_injected.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="${DOC_URL}"

USER root

# Add the polkadot binaries to the image
COPY ./polkadot ./polkadot-*-worker /usr/local/bin/
RUN chmod a+rwx /usr/local/bin/polkadot*

USER parity

# check if executable works in this container
RUN /usr/local/bin/polkadot --version
RUN /usr/local/bin/polkadot-execute-worker --version
RUN /usr/local/bin/polkadot-prepare-worker --version

EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/polkadot"]
