#
### planner
#
FROM rust as planner
WORKDIR /polkadot-simnet
# We only pay the installation cost once, 
# it will be cached from the second build onwards
# To ensure a reproducible build consider pinning 
# the cargo-chef version with `--version X.X.X`
# https://github.com/LukeMathWalker/cargo-chef
COPY substrate/ /polkadot-simnet
COPY polkadot/ /polkadot-simnet

RUN cargo install cargo-chef
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		ca-certificates \
    clang \
		curl \ 
    cmake \
		libssl1.1 \
    libssl-dev \
    pkg-config 

RUN export PATH="$PATH:$HOME/.cargo/bin" && \
   	rustup toolchain install nightly && \
   	rustup target add wasm32-unknown-unknown --toolchain nightly && \
   	rustup default stable 

WORKDIR /polkadot-simnet/polkadot
RUN cargo chef prepare  --recipe-path recipe.json

#
### cacher
#

FROM rust as cacher
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		ca-certificates \
    clang \
		curl \ 
    cmake \
		libssl1.1 \
    libssl-dev \
    pkg-config 

RUN export PATH="$PATH:$HOME/.cargo/bin" && \
   	rustup toolchain install nightly && \
   	rustup target add wasm32-unknown-unknown --toolchain nightly && \
   	rustup default stable 

WORKDIR /polkadot-simnet
COPY substrate/ /polkadot-simnet
COPY polkadot/ /polkadot-simnet
WORKDIR /polkadot-simnet/polkadot
RUN cargo install cargo-chef
COPY --from=planner /polkadot-simnet/polkadot/recipe.json recipe.json
ENV RUST_BACKTRACE=full

RUN export PATH="$PATH:$HOME/.cargo/bin" && \
   	rustup toolchain install nightly && \
   	rustup target add wasm32-unknown-unknown --toolchain nightly && \
   	rustup default stable 
RUN cargo chef cook --release --recipe-path recipe.json

#
### builder
#

FROM rust as builder
WORKDIR /polkadot-simnet
COPY substrate/ /polkadot-simnet
COPY polkadot/ /polkadot-simnet
# Copy over the cached dependencies
WORKDIR /polkadot-simnet/polkadot
COPY --from=cacher /polkadot-simnet/polkadot/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		ca-certificates \
    clang \
		curl \ 
    cmake \
		libssl1.1 \
    libssl-dev \
    pkg-config 

RUN export PATH="$PATH:$HOME/.cargo/bin" && \
   	rustup toolchain install nightly && \
   	rustup target add wasm32-unknown-unknown --toolchain nightly && \
   	rustup default stable && \
    cargo build -p polkadot-simnet --release 

#
### runtime
#

# Tini allows us to avoid several Docker edge cases, see https://github.com/krallin/tini.
FROM debian:buster-slim as runtime
RUN apt-get update && \
    apt-get install -y curl tini 

# Non-root user for security purposes.
#
# UIDs below 10,000 are a security risk, as a container breakout could result
# in the container being ran as a more privileged user on the host kernel with
# the same UID.
#
# Static GID/UID is also useful for chown'ing files outside the container where
# such a user does not exist.
RUN groupadd --gid 10001 nonroot && \
    useradd  --home-dir /home/nonroot \
             --create-home \
             --shell /bin/bash \
             --gid nonroot \
             --groups nonroot \
             --uid 10000 nonroot
WORKDIR /home/nonroot/polkadot-simnet
COPY substrate/ .
COPY polkadot/ .
COPY --from=builder /polkadot-simnet/polkadot/target/release/polkadot-simnet /usr/local/bin
RUN chown -R nonroot. /home/nonroot

# Use the non-root user to run our application
# Tell run test script that it runs in container
USER nonroot
# check if executable works in this container
RUN /usr/local/bin/polkadot-simnet --version
# Tini allows us to avoid several Docker edge cases, see https://github.com/krallin/tini.
ENTRYPOINT ["tini", "--", "/usr/local/bin/polkadot-simnet"]

