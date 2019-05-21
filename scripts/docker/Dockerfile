FROM debian:stretch-slim

# metadata
ARG VCS_REF
ARG BUILD_DATE

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="parity/polkadot" \
	io.parity.image.description="polkadot: a platform for web3" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/master/scripts/docker/Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# install tools and dependencies
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get upgrade -y && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		libssl1.1 \
		ca-certificates \
		curl && \
# apt cleanup
	apt-get autoremove -y && \
	apt-get clean && \
	find /var/lib/apt/lists/ -type f -not -name lock -delete; \
# add user
	useradd -m -u 1000 -U -s /bin/sh -d /polkadot polkadot

# add polkadot binary to docker image
COPY ./polkadot /usr/local/bin

USER polkadot

# check if executable works in this container
RUN /usr/local/bin/polkadot --version

EXPOSE 30333 9933 9944
VOLUME ["/polkadot"]

ENTRYPOINT ["/usr/local/bin/polkadot"]

