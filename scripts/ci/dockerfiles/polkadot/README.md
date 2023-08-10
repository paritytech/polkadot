# Self built Docker image

The Polkadot repo contains several options to build Docker images for Polkadot.

This folder contains a self-contained image that does not require a Linux pre-built binary.

Instead, building the image is possible on any host having docker installed and will
build Polkadot inside Docker. That also means that no Rust toolchain is required on the host
machine for the build to succeed.
