FROM paritytech/ci-linux:production as builder
LABEL description="This is the build stage. Here we create the binary."

ARG PROFILE=release
WORKDIR /app

COPY . /app

RUN cargo build --locked --$PROFILE --package staking-miner

# ===== SECOND STAGE ======

FROM debian:buster-slim
LABEL description="This is the 2nd stage: a very small image where we copy the binary."

ARG PROFILE=release
COPY --from=builder /app/target/$PROFILE/staking-miner /usr/local/bin

RUN useradd -m -u 1000 -U -s /bin/sh miner && \
	rm -rf /usr/bin /usr/sbin

USER miner

ENV SEED=""
ENV URI="wss://rpc.polkadot.io"
ENV RUST_LOG="info"

ENTRYPOINT [ "/usr/local/bin/staking-miner"]
