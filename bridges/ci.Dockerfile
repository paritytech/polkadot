# This file is a "runtime" part from a builder-pattern in Dockerfile, it's used in CI.
# The only different part is that the compilation happens externally,
# so COPY has a different source.
FROM ubuntu:20.04

# show backtraces
ENV RUST_BACKTRACE 1
ENV DEBIAN_FRONTEND=noninteractive

RUN set -eux; \
	apt-get update && \
	apt-get install -y --no-install-recommends \
        curl ca-certificates libssl-dev && \
    update-ca-certificates && \
	groupadd -g 1000 user && \
	useradd -u 1000 -g user -s /bin/sh -m user && \
	# apt clean up
	apt-get autoremove -y && \
	apt-get clean && \
	rm -rf /var/lib/apt/lists/*

# switch to non-root user
USER user

WORKDIR /home/user

ARG PROJECT=ethereum-poa-relay

COPY --chown=user:user ./${PROJECT} ./
COPY --chown=user:user ./bridge-entrypoint.sh ./

# check if executable works in this container
RUN ./${PROJECT} --version

ENV PROJECT=$PROJECT
ENTRYPOINT ["/home/user/bridge-entrypoint.sh"]

# metadata
ARG VCS_REF=master
ARG BUILD_DATE=""
ARG VERSION=""

LABEL org.opencontainers.image.title="${PROJECT}" \
    org.opencontainers.image.description="${PROJECT} - component of Parity Bridges Common" \
    org.opencontainers.image.source="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/ci.Dockerfile" \
    org.opencontainers.image.url="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/ci.Dockerfile" \
    org.opencontainers.image.documentation="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/README.md" \
    org.opencontainers.image.created="${BUILD_DATE}" \
    org.opencontainers.image.version="${VERSION}" \
    org.opencontainers.image.revision="${VCS_REF}" \
    org.opencontainers.image.authors="devops-team@parity.io" \
    org.opencontainers.image.vendor="Parity Technologies" \
    org.opencontainers.image.licenses="GPL-3.0 License"
