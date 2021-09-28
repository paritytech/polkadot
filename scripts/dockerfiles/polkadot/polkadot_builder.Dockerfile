FROM docker.io/paritytech/ci-linux:production as builder
LABEL io.parity.image.description="This is the build stage for Polkadot. Here we create the binary."

WORKDIR /polkadot

COPY . /polkadot

RUN cargo build --release --locked

# ===== SECOND STAGE ======

FROM docker.io/library/ubuntu:20.04
LABEL io.parity.image.description="Polkadot: a platform for web3. This is a self-buit multistage image."

COPY --from=builder /polkadot/target/release/polkadot /usr/local/bin

RUN useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot && \
	mkdir -p /polkadot/.local/share && \
	mkdir /data && \
	chown -R polkadot:polkadot /data && \
	ln -s /data /polkadot/.local/share/polkadot && \
	rm -rf /usr/bin /usr/sbin

USER polkadot
EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/polkadot"]
