# This is the build stage for Polkadot. Here we create the binary in a temporary image.
FROM docker.io/paritytech/ci-linux:production as builder

WORKDIR /polkadot
COPY . /polkadot

RUN cargo build --locked --release

# This is the 2nd stage: a very small image where we copy the Polkadot binary
# gcr.io/distroless/cc-debian11:nonroot
FROM gcr.io/distroless/cc-debian11@sha256:c2e1b5b0c64e3a44638e79246130d480ff09645d543d27e82ffd46a6e78a3ce3

LABEL description="Multistage Docker image for Polkadot: a platform for web3" \
	io.parity.image.type="builder" \
	io.parity.image.authors="chevdor@gmail.com, devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.description="Polkadot: a platform for web3" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/polkadot/polkadot_builder.Dockerfile" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

COPY --from=builder /polkadot/target/release/polkadot /polkadot

# check if executable works in this container
RUN ["/polkadot", "--version"]

USER 1000:1000
EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/polkadot", "-d", "/data"]
