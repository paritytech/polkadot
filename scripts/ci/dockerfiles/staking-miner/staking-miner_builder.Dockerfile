FROM paritytech/ci-linux:production as builder

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME="staking-miner"
ARG PROFILE=release

LABEL description="This is the build stage. Here we create the binary."

WORKDIR /app
COPY . /app
RUN cargo build --locked --$PROFILE --package staking-miner

# ===== SECOND STAGE ======

FROM docker.io/library/ubuntu:20.04
LABEL description="This is the 2nd stage: a very small image where we copy the binary."
LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="${IMAGE_NAME} for substrate based chains" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/${IMAGE_NAME}/${IMAGE_NAME}_builder.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

ARG PROFILE=release
COPY --from=builder /app/target/$PROFILE/staking-miner /usr/local/bin

RUN useradd -u 1000 -U -s /bin/sh miner && \
	rm -rf /usr/bin /usr/sbin

# show backtraces
ENV RUST_BACKTRACE 1

USER miner

ENV SEED=""
ENV URI="wss://rpc.polkadot.io"
ENV RUST_LOG="info"

# check if the binary works in this container
RUN /usr/local/bin/staking-miner --version

ENTRYPOINT [ "/usr/local/bin/staking-miner" ]
