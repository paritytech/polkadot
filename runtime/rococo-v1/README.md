# Rococo: v1

Rococo is a testnet runtime with no stability guarantees.

## How to run

> TODO: figure out how to run this properly.

### Alice

1. `cargo clean`
1. `cargo build --release`: produces `target/release/polkadot`
1. `target/release/polkadot --alice --tmp --rpc-external --ws-external --chain dev`
