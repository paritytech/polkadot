FROM debian:buster-slim

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME="staking-miner"

ENV SEED=""
ENV URI="wss://rpc.polkadot.io"
ENV RUST_LOG="info"

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="staking-miner for substrate based chains" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/docker/Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# install tools and dependencies
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		libssl1.1 \
		ca-certificates \
		curl && \
# apt cleanup
	apt-get autoremove -y && \
	apt-get clean && \
	find /var/lib/apt/lists/ -type f -not -name lock -delete; \
	useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot
	# && \
	# mkdir -p /data /polkadot/.local/share && \
	# chown -R polkadot:polkadot /data && \
	# ln -s /data /polkadot/.local/share/polkadot

# add binary to docker image
COPY ./staking-miner /usr/local/bin

USER polkadot

# check if executable works in this container
RUN /usr/local/bin/staking-miner --version

ENTRYPOINT [ "/usr/local/bin/staking-miner" ]
