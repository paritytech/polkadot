---
author: Polkadot developers
revision: 0.3.1
---

# Polkadot

Implementation of a https://polkadot.network node in Rust.

## NOTE

In 2018 we split our implementation of "Polkadot" from its platform-level component "Substrate". When we split them, we split the Polkadot code off into another repo (this repo), leaving the [**Substrate** repo][substrate-repo] to be what used to be Polkadot, along with its branches and releases.

We are actively building both Substrate and Polkadot, but things will be a little odd for a while.  If you see "substrate" and are wondering why you need it for Polkadot, now you know.

To connect on the "Kusama" canary network, you will want the `v0.7` code, which is in this **Polkadot** repo. To play on the ("Alexander") testnet, you'll want the PoC-4 code instead.  Note that PoC-3 uses the Alexander testnet, but will not be able to sync to the latest block.

* **Kusama** (né Kusama CC-3) is in this [**Polkadot**] repo `master` branch.

* **Kusama CC-2** is in this [**Polkadot**][polkadot-v0.6] repo branch `v0.6`.

* **Kusama CC-1** is in this [**Polkadot**][polkadot-v0.5] repo branch `v0.5`.

* **Polkadot PoC-4 "Alexander"** is in this [**Polkadot**][polkadot-v0.4] repo branch `v0.4`.

* **Polkadot PoC-3 "Alexander"** is in this [**Polkadot**][polkadot-v0.3] repo branch `v0.3`.

* **Polkadot PoC-2 "Krumme Lanke"** is in the [**Substrate**][substrate-v0.2] repo branch `v0.2`.

[substrate-repo]: https://github.com/paritytech/substrate
[polkadot-v0.6]: https://github.com/paritytech/polkadot/tree/v0.6
[polkadot-v0.5]: https://github.com/paritytech/polkadot/tree/v0.5
[polkadot-v0.4]: https://github.com/paritytech/polkadot/tree/v0.4
[polkadot-v0.3]: https://github.com/paritytech/polkadot/tree/v0.3
[substrate-v0.2]: https://github.com/paritytech/substrate/tree/v0.2

## To play

### Install Rust
If you'd like to play with Polkadot, you'll need to install a client like this
one. First, get Rust (1.39.0 or later) and the support software if you don't already have it:


```bash
curl https://sh.rustup.rs -sSf | sh
```

You may need to add Cargo's bin directoy to your PATH environment variable. Restarting your computer will do this for you automatically. Once done, finish installing the support software:

```bash
sudo apt install make clang pkg-config libssl-dev
```

If you already have Rust installed, make sure you're using the latest version by running:


```bash
rustup update
```

### Install "Kusama CC-3" Canary Network

Build Kusama by cloning this repository and running the following commands from the root directory of the repo:

```bash
git checkout master
./scripts/init.sh
cargo build --release
```

Connect to the global Kusama canary network by default by running:

```bash
./target/release/polkadot --name "hello world!"
```

You can see your node on [telemetry].

[telemetry]: https://telemetry.polkadot.io/#list/Kusama%20CC3

### Install PoC-4 on "Alexander" Testnet

Build Polkadot PoC-4 by cloning this repository and running the following commands from the root directory of the repo:

```bash
git checkout v0.4
./scripts/init.sh
./scripts/build.sh
cargo build --release
```

If you were previously running PoC-3 on this testnet, you may need to purge your chain data first:

```bash
./target/release/polkadot purge-chain
```

Finally, connect to the global "Alexander" testnet by default by running:

```bash
./target/release/polkadot
```

### Install PoC-2 "Krumme Lanke" Testnet

Install Polkadot PoC-2 and have a `polkadot` binary installed to your `PATH` with:

```
cargo install --git https://github.com/paritytech/substrate.git --branch v0.2 polkadot
```

Connect to the global "Krumme Lanke" testnet by default by running:

```bash
polkadot
```

### Install a custom Testnet version

You can run the following to get the very latest version of Polkadot, but these instructions will not work in that case.

```bash
cargo install --git https://github.com/paritytech/polkadot.git polkadot
```

If you want a specific version of Polkadot, say `0.2.5`, you may run

```bash
cargo install --git https://github.com/paritytech/substrate.git --tag v0.2.5 polkadot
```

### Obtaining DOTs

If you want to do anything on it (not that there's much to do), then you'll need to get an account and some Alexander or Krumme Lanke DOTs. Ask in the Polkadot watercooler ( https://riot.im/app/#/room/#polkadot-watercooler:matrix.org ) or get some from the Polkadot Testnet Faucet ( https://faucet.polkadot.network/ ).

### Development

You can run a simple single-node development "network" on your machine by
running in a terminal:

```bash
polkadot --dev
```

You can muck around by cloning and building the http://github.com/paritytech/polka-ui and http://github.com/paritytech/polkadot-ui or just heading to https://polkadot.js.org/apps and choose "Alexander (hosted by Parity)" from the Settings menu.


## Building

### Hacking on Polkadot

If you'd actually like hack on Polkadot, you can just grab the source code and build it. Ensure you have Rust and the support software installed:

```bash
curl https://sh.rustup.rs -sSf | sh
```

You may need to add Cargo's bin directoy to your PATH environment variable. Restarting your computer will do this for you automatically. Once done, finish installing the support software:

```bash
sudo apt install cmake pkg-config libssl-dev git clang
```

Then, grab the Polkadot source code:

```bash
git clone https://github.com/paritytech/polkadot.git
cd polkadot
```

Then build the code:

```bash
./scripts/init.sh   # Install WebAssembly. Update Rust
cargo build # Builds all native code
```

You can run the tests if you like:

```bash
cargo test --all
```

You can start a development chain with:

```bash
cargo run -- --dev
```

Detailed logs may be shown by running the node with the following environment variables set:

```bash
RUST_LOG=debug RUST_BACKTRACE=1 cargo run —- --dev
```

### Local Two-node Testnet

If you want to see the multi-node consensus algorithm in action locally, then you can create a local testnet. You'll need two terminals open. In one, run:

```bash
polkadot --chain=polkadot-local --alice -d /tmp/alice
```

And in the other, run:

```bash
polkadot --chain=polkadot-local --bob -d /tmp/bob --port 30334 --bootnodes '/ip4/127.0.0.1/tcp/30333/p2p/ALICE_BOOTNODE_ID_HERE'
```

Ensure you replace `ALICE_BOOTNODE_ID_HERE` with the node ID from the output of the first terminal.

### Using Docker
[Using Docker](doc/docker.md)

### Shell Completion
[Shell Completion](doc/shell-completion.md)

### Polkadot Networks
[Polkadot Networks](doc/networks/networks.md)

## Contributing

### Contributing Guidelines

[Contribution Guidelines](CONTRIBUTING.md)

### Contributor Code of Conduct

[Code of Conduct](CODE_OF_CONDUCT.md)

## License

[LICENSE](https://github.com/paritytech/polkadot/blob/master/LICENSE)

## Important Notice

https://polkadot.network/testnetdisclaimer
