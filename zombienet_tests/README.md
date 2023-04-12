# Zombienet tests

_The content of this directory is meant to be used by Parity's private CI/CD infrastructure with private tools. At the moment those tools are still early stage of development and we don't know if / when they will available for public use._

## Contents of this directory

`parachains`
    At the moment this directory only have one test related to parachains: `/parachains-smoke-test`, that check the parachain registration and the block height.

## Resources

* [zombienet repo](https://github.com/paritytech/zombienet)
* [zombienet book](https://paritytech.github.io/zombienet/)

## Running tests locally

To run any test locally use the native provider (`zombienet test -p native ...`) you need first build the binaries. They are:

* adder-collator -> polkadot/target/testnet/adder-collator
* malus -> polkadot/target/testnet/malus
* polkadot -> polkadot/target/testnet/polkadot
* polkadot-collator -> cumulus/target/release/polkadot-parachain
* undying-collator -> polkadot/target/testnet/undying-collator

To build them use:
* adder-collator -> `cargo build --profile testnet -p test-parachain-adder-collator`
* undying-collator -> `cargo build --profile testnet -p test-parachain-undying-collator`
* malus -> cargo build --profile testnet -p polkadot-test-malus
* polkadot (in polkadot repo) and polkadot-collator (in cumulus repo) -> `cargo build --profile testnet`

One solution is to use the `.set_env` file (from this directory) and fill the `CUSTOM_PATHS` before *source* it to patch the PATH of your system to find the binaries you just built.

E.g.:
```
$ cat .set_env
(...)
# by the order of this array
CUSTOM_PATHS=(
  "~/polkadot/target/release"
  "~/polkadot/target/testnet"
  "~/cumulus/target/release"
)
(...)

source .set_env
```

Then you have your `PATH` customized and ready to run `zombienet`.
 **NOTE**: You should need to do this ones per terminal session, since we are patching the `PATH` and re-exporting. **Or** you can also `source` this file in your `.bashrc` file to get executed automatically in each new session.

Example:

You can run a test locally by executing:
```sh
zombienet test -p native 0001-parachains-pvf.zndsl
```

## Questions / permissions

Ping in element Javier (@javier:matrix.parity.io) to ask questions or grant permission to run the test from your local setup.
