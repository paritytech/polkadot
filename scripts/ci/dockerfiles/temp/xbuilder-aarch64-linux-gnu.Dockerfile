FROM rust:latest

RUN apt update && apt upgrade -y
RUN apt install -y \
        g++-aarch64-linux-gnu libc6-dev-arm64-cross \
        pkg-config libssl-dev \
        protobuf-compiler clang make bsdmainutils && \
        rm -rf /var/lib/apt/lists/* /tmp/* && apt clean

RUN rustup target add aarch64-unknown-linux-gnu
RUN rustup toolchain install stable-aarch64-unknown-linux-gnu

WORKDIR /app

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="aarch64-linux-gnu-gcc" \
    CC_aarch64_unknown_linux_gnu="aarch64-linux-gnu-gcc" \
    CXX_aarch64_unknown_linux_gnu="aarch64-linux-gnu-g++" \
    BINDGEN_EXTRA_CLANG_ARGS="-I/usr/aarch64-linux-gnu/include/" \
    SKIP_WASM_BUILD=1

ENTRYPOINT ["cargo", "build", "--target", "aarch64-unknown-linux-gnu"]
