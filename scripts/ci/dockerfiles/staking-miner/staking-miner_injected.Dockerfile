# gcr.io/distroless/cc-debian11:nonroot
FROM gcr.io/distroless/cc-debian11@sha256:c2e1b5b0c64e3a44638e79246130d480ff09645d543d27e82ffd46a6e78a3ce3

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME="staking-miner"

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="${IMAGE_NAME} for substrate based chains" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/${IMAGE_NAME}/${IMAGE_NAME}_injected.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# add binary to docker image
COPY ./staking-miner /staking-miner

ENV SEED=""
ENV URI="wss://rpc.polkadot.io"
ENV RUST_LOG="info"

# check if the binary works in this container
RUN ["/staking-miner", "--version"]

ENTRYPOINT [ "/staking-miner" ]
