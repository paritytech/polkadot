FROM rust

WORKDIR /usr/src/polkadot-simnet
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
RUN ls -lrth /usr/src/polkadot-simnet
COPY substrate/ /usr/src/polkadot-simnet/substrate/
COPY polkadot/  /usr/src/polkadot-simnet/polkadot/
RUN ls -la /usr/src/polkadot-simnet
RUN ls -lrth /usr/src/polkadot-simnet/polkadot/
WORKDIR /usr/src/polkadot-simnet/polkadot
RUN ls -lrth /usr/src/polkadot-simnet/polkadot/


RUN cargo build -p polkadot-simnet --release
RUN cp /usr/src/polkadot-simnet/polkadot/target/release/polkadot-simnet /usr/local/bin

# check if executable works in this container
RUN /usr/local/bin/polkadot-simnet --version
CMD ["polkadot-simnet"]
