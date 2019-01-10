FROM ubuntu:xenial
LABEL maintainer "devops-team@parity.io"
LABEL description="Paritytech Polkadot Node Implementation"

RUN apt-get update && \
    apt-get upgrade -y && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y \
        libssl1.0.0 \
        ca-certificates \
        curl && \
    apt-get autoremove -y && \
    apt-get clean
    
RUN find /var/lib/apt/lists/ -type f -not -name lock -delete

COPY ./polkadot /usr/local/bin



RUN useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot
USER polkadot

ENV RUST_BACKTRACE 1

EXPOSE 30333 9933 9944
VOLUME ["/polkadot"]

ENTRYPOINT ["/usr/local/bin/polkadot"]

