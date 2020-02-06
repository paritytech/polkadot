FROM rust:buster as builder
LABEL maintainer "chevdor@gmail.com"
LABEL description="This is the build stage for Polkadot. Here we create the binary."

RUN apt update && \
    apt upgrade -y && \
    apt install -y cmake pkg-config libssl-dev git clang

ARG PROFILE=release
WORKDIR /polkadot

# initialize required build targets
# this is a separate build step for caching purposes
COPY scripts/init.sh scripts/init.sh
RUN scripts/init.sh

# fetch and build each module individually
# this maximizes the extent to which we can take advantage of caching: we've
# sorted the dependency list reverse-topographically, so changes to leaf
# packages should bust fewer caches than changes to root packages
COPY Cargo.* build.rs ./
COPY parachain parachain
RUN cargo build --$PROFILE -p polkadot-parachain
COPY test-parachains/halt test-parachains/halt
RUN cargo build --$PROFILE -p halt
COPY parachain/tests/adder parachain/tests/adder
COPY test-parachains/adder test-parachains/adder
RUN cargo build --$PROFILE -p adder
COPY primitives primitives
RUN cargo build --$PROFILE -p polkadot-primitives
COPY statement-table statement-table
RUN cargo build --$PROFILE -p polkadot-statement-table
COPY erasure-coding erasure-coding
RUN cargo build --$PROFILE -p polkadot-erasure-coding
COPY availability-store availability-store
RUN cargo build --$PROFILE -p polkadot-availability-store
COPY validation validation
RUN cargo build --$PROFILE -p polkadot-validation
COPY runtime/common runtime/common
RUN cargo build --$PROFILE -p polkadot-runtime-common
COPY runtime runtime
RUN cargo build --$PROFILE -p polkadot-runtime
COPY rpc rpc
RUN cargo build --$PROFILE -p polkadot-rpc
COPY network network
RUN cargo build --$PROFILE -p polkadot-network
COPY runtime/kusama runtime/kusama
RUN cargo build --$PROFILE -p kusama-runtime
COPY service service
RUN cargo build --$PROFILE -p polkadot-service
COPY cli cli
RUN cargo build --$PROFILE -p polkadot-cli
COPY src src
RUN cargo build --$PROFILE -p polkadot

# ===== SECOND STAGE ======

FROM phusion/baseimage:0.11
LABEL maintainer "chevdor@gmail.com"
LABEL description="This is the 2nd stage: a very small image where we copy the Polkadot binary."
ARG PROFILE=release

RUN mv /usr/share/ca* /tmp && \
    rm -rf /usr/share/*  && \
    mv /tmp/ca-certificates /usr/share/ && \
    rm -rf /usr/lib/python* && \
    useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot && \
    mkdir -p /polkadot/.local/share/polkadot && \
    chown -R polkadot:polkadot /polkadot/.local && \
    ln -s /polkadot/.local/share/polkadot /data && \
    rm -rf /usr/bin /usr/sbin

USER polkadot
EXPOSE 30333 9933 9944
VOLUME ["/data"]

COPY --from=builder /polkadot/target/$PROFILE/polkadot /usr/local/bin

CMD ["/usr/local/bin/polkadot"]
