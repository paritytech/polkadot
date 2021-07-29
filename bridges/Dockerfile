# Builds images used by the bridge.
#
# In particular, it can be used to build Substrate nodes and bridge relayers. The binary that gets
# built can be specified with the `PROJECT` build-arg. For example, to build the `substrate-relay`
# you would do the following:
#
# `docker build . -t local/substrate-relay --build-arg=PROJECT=substrate-relay`
#
# See the `deployments/README.md` for all the available `PROJECT` values.

FROM paritytech/bridges-ci:latest as builder
WORKDIR /parity-bridges-common

COPY . .

ARG PROJECT=ethereum-poa-relay
RUN cargo build --release --verbose -p ${PROJECT} && \
    strip ./target/release/${PROJECT}

# In this final stage we copy over the final binary and do some checks
# to make sure that everything looks good.
FROM ubuntu:20.04 as runtime

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

COPY --chown=user:user --from=builder /parity-bridges-common/target/release/${PROJECT} ./
COPY --chown=user:user --from=builder /parity-bridges-common/deployments/local-scripts/bridge-entrypoint.sh ./

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
    org.opencontainers.image.source="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/Dockerfile" \
    org.opencontainers.image.url="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/Dockerfile" \
    org.opencontainers.image.documentation="https://github.com/paritytech/parity-bridges-common/blob/${VCS_REF}/README.md" \
    org.opencontainers.image.created="${BUILD_DATE}" \
    org.opencontainers.image.version="${VERSION}" \
    org.opencontainers.image.revision="${VCS_REF}" \
    org.opencontainers.image.authors="devops-team@parity.io" \
    org.opencontainers.image.vendor="Parity Technologies" \
    org.opencontainers.image.licenses="GPL-3.0 License"
