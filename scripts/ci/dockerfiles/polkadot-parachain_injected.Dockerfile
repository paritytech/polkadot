# this file copies from scripts/ci/dockerfiles/Dockerfile and changes only the binary name
FROM docker.io/library/ubuntu:20.04

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG IMAGE_NAME

LABEL io.parity.image.authors="devops-team@parity.io" \
	io.parity.image.vendor="Parity Technologies" \
	io.parity.image.title="${IMAGE_NAME}" \
	io.parity.image.description="Injected adder-collator Docker image" \
	io.parity.image.source="https://github.com/paritytech/polkadot/blob/${VCS_REF}/scripts/ci/dockerfiles/collator_injected.Dockerfile" \
	io.parity.image.revision="${VCS_REF}" \
	io.parity.image.created="${BUILD_DATE}" \
	io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# install tools and dependencies
RUN apt-get update && \
	DEBIAN_FRONTEND=noninteractive apt-get install -y \
		libssl1.1 \
		ca-certificates && \
# apt cleanup
	apt-get autoremove -y && \
	apt-get clean && \
	find /var/lib/apt/lists/ -type f -not -name lock -delete; \
# add user and link ~/.local/share/adder-collator to /data
	useradd -m -u 1000 -U -s /bin/sh -d /adder-collator adder-collator && \
	mkdir -p /data /adder-collator/.local/share && \
	chown -R adder-collator:adder-collator /data && \
	ln -s /data /adder-collator/.local/share/polkadot

# add adder-collator binary to docker image
COPY ./adder-collator /usr/local/bin
COPY ./undying-collator /usr/local/bin

USER adder-collator

# check if executable works in this container
RUN /usr/local/bin/adder-collator --version
RUN /usr/local/bin/undying-collator --version

EXPOSE 30333 9933 9944
VOLUME ["/adder-collator"]

ENTRYPOINT ["/usr/local/bin/adder-collator"]
