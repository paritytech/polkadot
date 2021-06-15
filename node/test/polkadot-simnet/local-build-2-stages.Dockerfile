FROM rust:1.52.1

WORKDIR /usr/src/polkadot
COPY . .

RUN cargo build -p polkadot-simnet --release

CMD ["polkadot-simnet"]
