# Staking Miner

The staking miner connects to a specified chains and keeps listening to new Signed phase of the [pallet-election-provider-multi-phase](https://crates.parity.io/pallet_election_provider_multi_phase/index.html) in order to submit solutions to the NPoS election. When the correct time comes, it computes its solution and submit it to the chain.
The default miner algorithm is [sequential-phragmen](https://crates.parity.io/sp_npos_elections/phragmen/fn.seq_phragmen_core.html)] with a configurable number of balancing iterations that improve the score.

Running the staking-miner requires passing the seed of a funded account in order to pay the fees for the transactions that will be sent. 
The same account's balance is used to reserve deposits as well. The best solution in each round is rewarded. All correct solutions will get their bond back. Any invalid solution will lose their bond.

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

### Building

You can also use docker on a Linux host from the root of the Polkadot repository after building the binary (see above):
```
docker build -t staking-miner -f scripts/docker/staking_miner-injected.Dockerfile target/release
```

### Running

A Docker container, especially one holding one of your `SEED` should be kept secure. It is recommended running this container in `read-only` mode:

```
docker run --rm -it \
    --name staking-miner \
    --read-only \
    -v /tmp/seed.txt:/seed.txt \
    staking-miner \
        --account-seed /seed.txt \
        --uri wss://your-nodes:9944 \
        dry-run
```

There is a [pending PR](https://github.com/paritytech/polkadot/pull/3680) that removes the need to pass the seed as a file. The command will then become:
```
docker run --rm -it \
    --name staking-miner \
    --read-only \
    -e SEED=0x1234... \
    -e URI=wss://your-nodes:9944 \
    staking-miner dry-run
```
