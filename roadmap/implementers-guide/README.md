# The Polkadot Parachain Host Implementers' Guide

The implementers' guide is compiled from several source files with [`mdBook`](https://github.com/rust-lang/mdBook).

## Hosted build

This is avalible at https://paritytech.github.io/polkadot/book/

## Local build

To view it locally from the repo root:

Ensure graphviz is installed:
```sh
brew install graphviz # for macOS
sudo apt-get install graphviz # for Ubuntu/Debian
```

Then install and build the book:

```sh
cargo install mdbook mdbook-linkcheck mdbook-graphviz mdbook-mermaid
mdbook serve roadmap/implementers-guide
open http://localhost:3000
```

## Specification

See also the Polkadot specificaton [hosted](https://spec.polkadot.network/), and it's [source](https://github.com/w3f/polkadot-spec)).
