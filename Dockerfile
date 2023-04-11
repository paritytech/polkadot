FROM ubuntu:22.04

WORKDIR /bin

# Install root certs, see: https://github.com/paritytech/substrate/issues/9984
RUN apt update && \
    apt install -y ca-certificates && \
    update-ca-certificates && \
    apt remove ca-certificates -y && \
    rm -rf /var/lib/apt/lists/*

ADD --chown=777 polkadot .

# Fail on build, if unable to run node
RUN polkadot --version

EXPOSE 3033 9933 9944 9615
VOLUME [ "/config" ]

ENTRYPOINT [ "polkadot" ]