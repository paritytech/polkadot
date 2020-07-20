# Polkadot

Implementation of a https://polkadot.network node in Rust based on the Substrate framework.

> **NOTE:** In 2018, we split our implementation of "Polkadot" from its development framework
> "Substrate". See the [Substrate][substrate-repo] repo for git history prior to 2018.

[substrate-repo]: https://github.com/paritytech/substrate

This repo contains runtimes for the Polkadot, Kusama, and Westend networks. The README provides
information about installing the `polkadot` binary and developing on the codebase. For more
specific guides, like how to be a validator, see the
[Polkadot Wiki](https://wiki.polkadot.network/docs/en/).

## Building

### Use a Provided Binary

If you want to connect to one of the networks supported by this repo, you can go to the latest
release and download the binary that is provided.

### Install via Cargo

If you want to install Polkadot in your PATH, you can do so with with:

```bash
cargo install --force --git https://github.com/paritytech/polkadot --tag <version> polkadot
```

### Build from Source

If you'd like to build from source, first install Rust. You may need to add Cargo's bin directory
to your PATH environment variable. Restarting your computer will do this for you automatically.

```bash
curl https://sh.rustup.rs -sSf | sh
```

If you already have Rust installed, make sure you're using the latest version by running:

```bash
rustup update
```

Once done, finish installing the support software:

```bash
sudo apt install make clang pkg-config libssl-dev
```

Build the client by cloning this repository and running the following commands from the root
directory of the repo:

```bash
git checkout <latest tagged release>
./scripts/init.sh
cargo build --release
```

## Networks

This repo supports runtimes for Polkadot, Kusama, and Westend.

### Connect to Polkadot Mainnet

Connect to the global Polkadot Mainnet network by running:

```bash
./target/release/polkadot --chain=polkadot
```

You can see your node on [telemetry] (set a custom name with `--name "my custom name"`).

[telemetry]: https://telemetry.polkadot.io/#list/Polkadot

### Connect to the "Kusama" Canary Network

Connect to the global Kusama canary network by running:

```bash
./target/release/polkadot --chain=kusama
```

You can see your node on [telemetry] (set a custom name with `--name "my custom name"`).

[telemetry]: https://telemetry.polkadot.io/#list/Kusama

### Connect to the Westend Testnet

Connect to the global Westend testnet by running:

```bash
./target/release/polkadot --chain=westend
```

You can see your node on [telemetry] (set a custom name with `--name "my custom name"`).

[telemetry]: https://telemetry.polkadot.io/#list/Westend

### Obtaining DOTs

If you want to do anything on Polkadot, Kusama, or Westend, then you'll need to get an account and
some DOT, KSM, or WND tokens, respectively. See the
[claims instructions](https://claims.polkadot.network/) for Polkadot if you have DOTs to claim. For
Westend's WND tokens, see the faucet
[instructions](https://wiki.polkadot.network/docs/en/learn-DOT#getting-westies) on the Wiki.

## Hacking on Polkadot

If you'd actually like hack on Polkadot, you can grab the source code and build it. Ensure you have
Rust and the support software installed. This script will install or update Rust and install the
required dependencies (this may take up to 30 minutes on Mac machines):

```bash
curl https://getsubstrate.io -sSf | bash -s -- --fast
```

Then, grab the Polkadot source code:

```bash
git clone https://github.com/paritytech/polkadot.git
cd polkadot
```

Then build the code. You will need to build in release mode (`--release`) to start a network. Only
use debug mode for development (faster compile times for development and testing).

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
RUST_LOG=debug RUST_BACKTRACE=1 cargo run -- --dev
```

### Development

You can run a simple single-node development "network" on your machine by running:

```bash
polkadot --dev
```

You can muck around by heading to https://polkadot.js.org/apps and choose "Local Node" from the
Settings menu.

### Local Two-node Testnet

If you want to see the multi-node consensus algorithm in action locally, then you can create a
local testnet. You'll need two terminals open. In one, run:

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

## Contributing

### Contributing Guidelines

[Contribution Guidelines](CONTRIBUTING.md)

### Contributor Code of Conduct

[Code of Conduct](CODE_OF_CONDUCT.md)

## License

Polkadot is [GPL 3.0 licensed](LICENSE).

## Important Notice

https://polkadot.network/testnetdisclaimer
