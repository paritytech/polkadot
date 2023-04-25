# musl-builder

musl-builder is a tool that integrates the process of building the musl binary
of your project into the main `cargo` build process.

<!-- TODO: add project setup -->

## Prerequisites

musl-builder requires a musl toolchain like `x86_64-unknown-linux-musl` to be installed:

```sh
rustup target add x86_64-unknown-linux-musl
```

<!-- TODO: What to do with License? (wasm-builder is Apache-2.0) -->
