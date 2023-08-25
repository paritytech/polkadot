# This is the build stage for Polkadot. Here we create the binary in a temporary image.
FROM docker.io/paritytech/ci-linux:production as builder

ARG PROFILE=production

WORKDIR /polkadot
COPY . /polkadot

RUN cargo build --locked --profile ${PROFILE}

# This is the 2nd stage: a very small image where we copy the Polkadot binary."
FROM docker.io/parity/base-bin:latest

USER root
ARG PROFILE=production

LABEL description="Multistage Docker image for Polkadot: a platform for web3" \
	io.parity.image.type="builder" \
	io.parity.image.authors="chevdor@gmail.com, devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.description="Polkadot: a platform for web3" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/polkadot/polkadot_builder.Dockerfile" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

RUN mkdir -p /usr/local/bin

COPY --from=builder /polkadot/target/${PROFILE}/polkadot-prepare-worker /usr/local/bin
COPY --from=builder /polkadot/target/${PROFILE}/polkadot-execute-worker /usr/local/bin
COPY --from=builder /polkadot/target/${PROFILE}/polkadot /usr/local/bin

USER parity

# check if executable works in this container
RUN /usr/local/bin/polkadot --version

EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/polkadot"]

# We only show the version by default
CMD ["--version"]
