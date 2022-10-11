FROM docker.io/library/ubuntu:20.04

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG POLKADOT_VERSION
ARG POLKADOT_GPGKEY=9D4B2B6EB8F97156D19669A9FF0812D491B96798
ARG GPG_KEYSERVER="keyserver.ubuntu.com"

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="parity/polkadot" \
	io.parity.image.description="Polkadot: a platform for web3. This is the official Parity image with an injected binary." \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/polkadot_injected_release.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# install tools and dependencies
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
		libssl1.1 \
		ca-certificates \
		gnupg && \
	useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot && \
# add repo's gpg keys and install the published polkadot binary
	gpg --keyserver ${GPG_KEYSERVER} --recv-keys ${POLKADOT_GPGKEY} && \
	gpg --export ${POLKADOT_GPGKEY} > /usr/share/keyrings/parity.gpg && \
	echo 'deb [signed-by=/usr/share/keyrings/parity.gpg] https://releases.parity.io/deb release main' > /etc/apt/sources.list.d/parity.list && \
	apt-get update && \
	apt-get install -y --no-install-recommends polkadot=${POLKADOT_VERSION#?} && \
# apt cleanup
	apt-get autoremove -y && \
	apt-get clean && \
	rm -rf /var/lib/apt/lists/* ; \
	mkdir -p /data /polkadot/.local/share && \
	chown -R polkadot:polkadot /data && \
	ln -s /data /polkadot/.local/share/polkadot

USER polkadot

# check if executable works in this container
RUN /usr/bin/polkadot --version

EXPOSE 30333 9933 9944
VOLUME ["/polkadot"]

ENTRYPOINT ["/usr/bin/polkadot"]
