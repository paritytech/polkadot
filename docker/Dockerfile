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

# fetch and build dependencies
# this is a separate build step for caching purposes
COPY Cargo.* build.rs ./
COPY availability-store/Cargo.toml ./availability-store/
COPY cli/Cargo.toml cli/
COPY collator/Cargo.toml collator/
COPY erasure-coding/Cargo.toml ./erasure-coding/
COPY network/Cargo.toml network/
COPY parachain/Cargo.toml parachain/
COPY primitives/Cargo.toml ./primitives/
COPY rpc/Cargo.toml rpc/
COPY runtime/common/Cargo.toml runtime/common/
COPY runtime/kusama/Cargo.toml runtime/kusama/build.rs runtime/kusama/
COPY runtime/polkadot/Cargo.toml runtime/polkadot/build.rs runtime/polkadot/
COPY service/Cargo.toml service/
COPY statement-table/Cargo.toml statement-table/
COPY test-parachains/adder/Cargo.toml test-parachains/adder/build.rs test-parachains/adder/
COPY test-parachains/adder/collator/Cargo.toml test-parachains/adder/collator/
COPY test-parachains/halt/Cargo.toml test-parachains/halt/build.rs test-parachains/halt/
COPY validation/Cargo.toml validation/
RUN mkdir src && touch src/lib.rs && \
    mkdir availability-store/src && touch availability-store/src/lib.rs && \
    mkdir cli/src && touch cli/src/lib.rs && \
    mkdir collator/src && touch collator/src/lib.rs && \
    mkdir erasure-coding/src && touch erasure-coding/src/lib.rs && \
    mkdir network/src && touch network/src/lib.rs && \
    mkdir parachain/src && touch parachain/src/lib.rs && \
    mkdir primitives/src && touch primitives/src/lib.rs && \
    mkdir rpc/src && touch rpc/src/lib.rs && \
    mkdir runtime/common/src && touch runtime/common/src/lib.rs && \
    mkdir runtime/kusama/src && touch runtime/kusama/src/lib.rs && \
    mkdir runtime/polkadot/src && touch runtime/polkadot/src/lib.rs && \
    mkdir service/src && touch service/src/lib.rs && \
    mkdir statement-table/src && touch statement-table/src/lib.rs && \
    mkdir test-parachains/adder/src && touch test-parachains/adder/src/lib.rs && \
    mkdir test-parachains/adder/collator/src && touch test-parachains/adder/collator/src/lib.rs && \
    mkdir test-parachains/halt/src && touch test-parachains/halt/src/lib.rs && \
    mkdir validation/src && touch validation/src/lib.rs && \
    echo 'fn main(){}' > src/main.rs && \
    echo 'fn main(){}' > test-parachains/adder/collator/src/main.rs && \
    SKIP_WASM_BUILD=1 cargo build --${PROFILE} && \
    find . -name lib.rs -o -name main.rs -delete

# now build the actual project
COPY . .
# we build each module independently, in reverse topographic order, for maximum caching
RUN cargo build --$PROFILE -p polkadot-parachain
RUN cargo build --$PROFILE -p halt
RUN cargo build --$PROFILE -p adder
RUN cargo build --$PROFILE -p polkadot-primitives
RUN cargo build --$PROFILE -p polkadot-statement-table
RUN cargo build --$PROFILE -p polkadot-erasure-coding
RUN cargo build --$PROFILE -p polkadot-availability-store
RUN cargo build --$PROFILE -p polkadot-validation
RUN cargo build --$PROFILE -p polkadot-runtime-common
RUN cargo build --$PROFILE -p polkadot-runtime
RUN cargo build --$PROFILE -p polkadot-rpc
RUN cargo build --$PROFILE -p polkadot-network
RUN cargo build --$PROFILE -p kusama-runtime
RUN cargo build --$PROFILE -p polkadot-service
RUN cargo build --$PROFILE -p polkadot-cli
RUN cargo build --$PROFILE -p polkadot

# ===== SECOND STAGE ======

FROM phusion/baseimage:0.11
LABEL maintainer "chevdor@gmail.com"
LABEL description="This is the 2nd stage: a very small image where we copy the Polkadot binary."
ARG PROFILE=release
COPY --from=builder /polkadot/target/$PROFILE/polkadot /usr/local/bin

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

CMD ["/usr/local/bin/polkadot"]
