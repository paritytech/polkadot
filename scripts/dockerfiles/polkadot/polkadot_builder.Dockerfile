# This is the build stage for Polkadot. Here we create the binary in a temporary image.
FROM docker.io/paritytech/ci-linux:production as builder

ARG PROFILE=release

WORKDIR /polkadot

COPY . /polkadot

RUN cargo build --locked --$PROFILE


# This is the 2nd stage: a very small image where we copy the Polkadot binary."
FROM docker.io/library/ubuntu:20.04

LABEL description="Multistage Docker image for Polkadot: a platform for web3" \
    io.parity.image.type="builder" \
    io.parity.image.authors="chevdor@gmail.com, devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.description="Polkadot: a platform for web3" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/docker/Dockerfile" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

ARG PROFILE=release

COPY --from=builder /polkadot/target/$PROFILE/polkadot /usr/local/bin

RUN useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot && \
	mkdir -p /data /polkadot/.local/share && \
	chown -R polkadot:polkadot /data && \
	ln -s /data /polkadot/.local/share/polkadot && \
	rm -rf /usr/bin /usr/sbin

USER polkadot

# check if executable works in this container
RUN /usr/local/bin/polkadot --version

EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/polkadot"]
