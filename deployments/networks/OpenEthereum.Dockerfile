FROM ubuntu:xenial AS builder

# show backtraces
ENV RUST_BACKTRACE 1

ENV LAST_DEPS_UPDATE 2020-06-19

# install tools and dependencies
RUN set -eux; \
	apt-get update && \
	apt-get install -y file curl jq ca-certificates && \
	apt-get install -y cmake pkg-config libssl-dev git clang libclang-dev

ENV LAST_CERTS_UPDATE 2020-06-19

RUN update-ca-certificates && \
	curl https://sh.rustup.rs -sSf | sh -s -- -y

ENV PATH="/root/.cargo/bin:${PATH}"
ENV LAST_RUST_UPDATE="2020-09-09"
RUN rustup update stable && \
	rustup install nightly && \
	rustup target add wasm32-unknown-unknown --toolchain nightly

RUN rustc -vV && \
    cargo -V && \
    gcc -v && \
    g++ -v && \
    cmake --version

WORKDIR /openethereum

### Build from the repo
ARG ETHEREUM_REPO=https://github.com/paritytech/openethereum.git
ARG ETHEREUM_HASH=344991dbba2bc8657b00916f0e4b029c66f159e8
RUN git clone $ETHEREUM_REPO /openethereum && git checkout $ETHEREUM_HASH

### Build locally. Make sure to set the CONTEXT to main directory of the repo.
# ADD openethereum /openethereum

WORKDIR /parity-bridges-common

### Build from the repo
# Build using `master` initially.
ARG BRIDGE_REPO=https://github.com/paritytech/parity-bridges-common
RUN git clone $BRIDGE_REPO /parity-bridges-common && git checkout master

WORKDIR /openethereum
RUN cargo build --release --verbose || true

# Then rebuild by switching to a different branch to only incrementally
# build the changes.
WORKDIR /parity-bridges-common
ARG BRIDGE_HASH=master
RUN git checkout . && git fetch && git checkout $BRIDGE_HASH
### Build locally. Make sure to set the CONTEXT to main directory of the repo.
# ADD . /parity-bridges-common

WORKDIR /openethereum
RUN cargo build --release --verbose
RUN strip ./target/release/openethereum

FROM ubuntu:xenial

# show backtraces
ENV RUST_BACKTRACE 1

RUN set -eux; \
	apt-get update && \
	apt-get install -y curl

RUN groupadd -g 1000 openethereum \
  && useradd -u 1000 -g openethereum -s /bin/sh -m openethereum

# switch to user openethereum here
USER openethereum

WORKDIR /home/openethereum

COPY --chown=openethereum:openethereum --from=builder /openethereum/target/release/openethereum ./
# Solve issues with custom --keys-path
RUN mkdir -p ~/.local/share/io.parity.ethereum/keys/
# check if executable works in this container
RUN ./openethereum --version

EXPOSE 8545 8546 30303/tcp 30303/udp

HEALTHCHECK --interval=2m --timeout=5s \
  CMD curl -f http://localhost:8545/api/health || exit 1

ENTRYPOINT ["/home/openethereum/openethereum"]
