FROM debian:buster-slim AS rust-env
# Set environment variables
ENV RUSTUP_HOME="/opt/rust"
ENV CARGO_HOME="/opt/rust"
ENV CARGO_TARGET_DIR="/opt/rust-target"
ENV PATH="$PATH:$RUSTUP_HOME/bin"
ENV RUST_VERSION=nightly-2021-03-11
ARG POLKADOT_COMMIT
# Install dependencies
RUN apt-get update && \
    apt-get install --no-install-recommends -y \
        curl ca-certificates \
        build-essential git clang libclang-dev pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*
# Install rust
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --no-modify-path --default-toolchain ${RUST_VERSION} && \
    rustup default ${RUST_VERSION} && \
    rustup target add wasm32-unknown-unknown && \
    command -v wasm-gc || \
        cargo install --git https://github.com/alexcrichton/wasm-gc --force
# Build project
RUN git clone https://github.com/soramitsu/fearless-polkadot.git  /polkadot
WORKDIR /polkadot
RUN git checkout ${POLKADOT_COMMIT}
RUN cargo build --release

FROM debian:buster-slim
# add polkadot binary to docker image
COPY --from=rust-env /opt/rust-target/release/polkadot /usr/local/bin
# install tools and dependencies
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get upgrade -y && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		libssl1.1 \
		ca-certificates \
		curl bash && \
# apt cleanup
	apt-get autoremove -y && \
	apt-get clean && \
	find /var/lib/apt/lists/ -type f -not -name lock -delete
# add user
RUN useradd -m -u 10000 -U -s /bin/bash polkadot
RUN chown -R polkadot:polkadot /home/polkadot && \
    mkdir /chain && \
    chown 10000:10000 /chain
USER polkadot
# check if executable works in this container
RUN /usr/local/bin/polkadot --version
EXPOSE 30333 9933 9944
ENTRYPOINT ["/usr/local/bin/polkadot"]