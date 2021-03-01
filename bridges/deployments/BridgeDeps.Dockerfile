# Image with dependencies required to build projects from the bridge repo.
#
# This image is meant to be used as a building block when building images for
# the various components in the bridge repo, such as nodes and relayers.
FROM ubuntu:xenial

ENV LAST_DEPS_UPDATE 2020-12-21

RUN set -eux; \
	apt-get update && \
	apt-get install -y curl ca-certificates && \
	apt-get install -y cmake pkg-config libssl-dev git clang libclang-dev

ENV LAST_CERTS_UPDATE 2020-12-21

RUN update-ca-certificates && \
	curl https://sh.rustup.rs -sSf | sh -s -- -y

ENV PATH="/root/.cargo/bin:${PATH}"
ENV LAST_RUST_UPDATE 2020-12-21

RUN rustup update stable && \
	rustup install nightly && \
	rustup target add wasm32-unknown-unknown --toolchain nightly

RUN rustc -vV && \
    cargo -V && \
    gcc -v && \
    g++ -v && \
    cmake --version

ENV RUST_BACKTRACE 1
