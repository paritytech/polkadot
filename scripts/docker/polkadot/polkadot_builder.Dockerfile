FROM docker.io/paritytech/ci-linux:production as builder
LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="builder" \
	io.parity.image.description="This is the build stage for Polkadot. Here we create the binary." \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/master/scripts/docker/polkadot/polkadot_builder.Dockerfile" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/scripts/docker/polkadot/README.md"

ARG PROFILE=release
WORKDIR /polkadot

COPY . /polkadot

RUN cargo build --$PROFILE

# ===== SECOND STAGE ======

FROM docker.io/library/ubuntu:20.04
LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="builder" \
	io.parity.image.description="This is the 2nd stage: a very small image where we copy the Polkadot binary." \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/master/scripts/docker/polkadot/polkadot_builder.Dockerfile" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/scripts/docker/polkadot/README.md"

ARG PROFILE=release
COPY --from=builder /polkadot/target/$PROFILE/polkadot /usr/local/bin

RUN useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot && \
	mkdir -p /polkadot/.local/share && \
	mkdir /data && \
	chown -R polkadot:polkadot /data && \
	ln -s /data /polkadot/.local/share/polkadot && \
	rm -rf /usr/bin /usr/sbin

USER polkadot
EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

CMD ["/usr/local/bin/polkadot"]
