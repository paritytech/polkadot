FROM docker.io/alpine:3.16.0 as verifier

# metadata
ARG VCS_REF
ARG BUILD_DATE
ARG POLKADOT_VERSION
ARG POLKADOT_GPGKEY=9D4B2B6EB8F97156D19669A9FF0812D491B96798
ARG GPG_KEYSERVER="keyserver.ubuntu.com"

# install tools
RUN set -eu -o pipefail && \
	apk add --no-cache curl gnupg && \
	curl -fsSL -o polkadot https://releases.parity.io/polkadot/x86_64-debian:stretch/${POLKADOT_VERSION}/polkadot && \
	curl -fsSL -o polkadot.sha256 https://releases.parity.io/polkadot/x86_64-debian:stretch/${POLKADOT_VERSION}/polkadot.sha256 && \
	curl -fsSL -o polkadot.asc https://releases.parity.io/polkadot/x86_64-debian:stretch/${POLKADOT_VERSION}/polkadot.asc && \
	sha256sum -c polkadot.sha256 && \
	gpg --keyserver ${GPG_KEYSERVER} --recv-keys ${POLKADOT_GPGKEY} && \
	gpg --verify polkadot.asc polkadot && \
	chmod +x polkadot

# This is the 2nd stage: a very small image where we copy the Polkadot binary
# gcr.io/distroless/cc-debian11:nonroot
FROM gcr.io/distroless/cc-debian11@sha256:c2e1b5b0c64e3a44638e79246130d480ff09645d543d27e82ffd46a6e78a3ce3

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

COPY --from=verifier /polkadot /polkadot

# check if executable works in this container
RUN ["/polkadot", "--version"]
USER 1000:1000
EXPOSE 30333 9933 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/polkadot", "-d", "/data"]
