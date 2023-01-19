# Zombienet tests

_The content of this directory is meant to be used by Parity's private CI/CD infrastructure with private tools. At the moment those tools are still early stage of development and we don't know if / when they will available for public use._

## Contents of this directory

`parachains`
    At the moment this directory only have one test related to parachains: `/parachains-smoke-test`, that check the parachain registration and the block height.

## Resources

* [zombienet repo](https://github.com/paritytech/zombienet)
* [zombienet book](https://paritytech.github.io/zombienet/)

## Running tests locally

To run any test locally use the native provider (`zombienet test -p native ...`) and set `ZOMBIENET_NATIVE_BINARY_PREFIX` environment variable with a full path to a directory containing polkadot/cumulus binaries. E.g. `ZOMBIENET_NATIVE_BINARY_PREFIX=/home/user/local-run/`. **The path should contain a trailing slash!**

You need the following binaries:
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

One solution is to have a directory with symlinks to the corresponding binaries in your source tree. E.g.:
```
$ ls -l
total 24
lrwxrwxrwx. 1 ceco ceco  53 Jan 19 15:03 adder-collator -> /home/ceco/src/polkadot/target/testnet/adder-collator
-rw-r--r--. 1 ceco ceco 221 Jan 18 16:35 build_cmds.txt
lrwxrwxrwx. 1 ceco ceco  44 Jan 18 16:47 malus -> /home/user/src/polkadot/target/testnet/malus
lrwxrwxrwx. 1 ceco ceco  47 Jan 18 11:33 polkadot -> /home/user/src/polkadot/target/testnet/polkadot
lrwxrwxrwx. 1 ceco ceco  56 Jan 19 15:53 polkadot-collator -> /home/user/src/cumulus/target/release/polkadot-parachain
lrwxrwxrwx. 1 ceco ceco  55 Jan 18 13:58 undying-collator -> /home/user/src/polkadot/target/testnet/undying-collator
```
And set this directory in `ZOMBIENET_NATIVE_BINARY_PREFIX`. This way you won't have to copy files on each rebuild.
## Questions / permissions

Ping in element Javier (@javier:matrix.parity.io) to ask questions or grant permission to run the test from your local setup.
