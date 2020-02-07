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

# now build the actual project
COPY . .
RUN cargo build --$PROFILE

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

CMD ["/usr/local/bin/polkadot", "--rpc-external"]
