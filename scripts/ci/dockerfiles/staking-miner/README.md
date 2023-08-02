# staking-miner container image

## Build using the Builder

```
./build.sh
```

## Build the injected Image

You first need a valid Linux binary to inject. Let's assume this binary is located in `BIN_FOLDER`.

```
./build-injected.sh "$BIN_FOLDER"
```

## Test

Here is how to test the image. We can generate a valid seed but the staking-miner will quickly notice that our
account is not funded and "does not exist".

You may pass any ENV supported by the binary and must provide at least a few such as `SEED` and `URI`:
```
ENV SEED=""
ENV URI="wss://rpc.polkadot.io:443"
ENV RUST_LOG="info"
```

```
export SEED=$(subkey generate -n polkadot --output-type json  | jq -r .secretSeed)
podman run --rm -it \
    -e URI="wss://rpc.polkadot.io:443" \
    -e RUST_LOG="info" \
    -e SEED \
    localhost/parity/staking-miner \
        dry-run seq-phragmen
```
