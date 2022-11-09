# Staking Miner

Substrate chains validators compute a basic solution for the NPoS election. The optimization of the solution is computing-intensive and can be delegated to the `staking-miner`. The `staking-miner` does not act as validator and focuses solely on the optimization of the solution.

The staking miner connects to a specified chain and keeps listening to new Signed phase of the [pallet-election-provider-multi-phase](https://crates.parity.io/pallet_election_provider_multi_phase/index.html) in order to submit solutions to the NPoS election. When the correct time comes, it computes its solution and submit it to the chain.
The default miner algorithm is [sequential-phragmen](https://crates.parity.io/sp_npos_elections/phragmen/fn.seq_phragmen_core.html)] with a configurable number of balancing iterations that improve the score.

Running the staking-miner requires passing the seed of a funded account in order to pay the fees for the transactions that will be sent. The same account's balance is used to reserve deposits as well. The best solution in each round is rewarded. All correct solutions will get their bond back. Any invalid solution will lose their bond.

You can check the help with:
```
staking-miner --help
```

## Building

You can build from the root of the Polkadot repository using:
```
cargo build --release --locked --package staking-miner
```

## Docker

There are 2 options to build a staking-miner Docker image:
- injected binary: the binary is first built on a Linux host and then injected into a Docker base image. This method only works if you have a Linux host or access to a pre-built binary from a Linux host.
- multi-stage: the binary is entirely built within the multi-stage Docker image. There is no requirement on the host in terms of OS and the host does not even need to have any Rust toolchain installed.

### Building the injected image

First build the binary as documented [above](#building).
You may then inject the binary into a Docker base image from the root of the Polkadot repository:
```
docker build -t staking-miner -f scripts/ci/dockerfiles/staking-miner/staking-miner_injected.Dockerfile target/release
```

### Building the multi-stage image

Unlike the injected image that requires a Linux pre-built binary, this option does not requires a Linux host, nor Rust to be installed.
The trade-off however is that it takes a little longer to build and this option is less ideal for CI tasks.
You may build the multi-stage image the root of the Polkadot repository with:
```
docker build -t staking-miner -f scripts/ci/dockerfiles/staking-miner/staking-miner_builder.Dockerfile .
```

### Running

A Docker container, especially one holding one of your `SEED` should be kept as secure as possible.
While it won't prevent a malicious actor to read your `SEED` if they gain access to your container, it is nonetheless recommended running this container in `read-only` mode:

```
# The following line starts with an extra space on purpose:
 SEED=0x1234...

docker run --rm -it \
    --name staking-miner \
    --read-only \
    -e RUST_LOG=info \
    -e SEED=$SEED \
    -e URI=wss://your-node:9944 \
    staking-miner dry-run
```

### Test locally

1. Modify `EPOCH_DURATION_IN_SLOTS` and `SessionsPerEra` to force an election
   more often than once per day.
2. $ polkadot --chain polkadot-dev --tmp --alice --execution Native -lruntime=debug --offchain-worker=Always --ws-port 9999
3. $ staking-miner --uri ws://localhost:9999 --seed //Alice monitor phrag-mms
