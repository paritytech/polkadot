# binary-builder

binary-builder is a tool that integrates the process of building the binary of a
crate into the main `cargo` build process. The binary can then be embedded into
other crates and extracted at runtime.

<!-- TODO: add project setup -->

## Prerequisites

binary-builder requires the chosen toolchain (e.g. `x86_64-unknown-linux-musl`)
to be installed:

```sh
rustup target add x86_64-unknown-linux-musl
```

<!-- TODO: What to do with License? (wasm-builder is Apache-2.0) -->
