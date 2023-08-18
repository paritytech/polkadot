# malus

Create nemesis nodes with alternate, at best faulty, at worst intentionally destructive behavior traits.

The first argument determines the behavior strain. The currently supported are:

* `suggest-garbage-candidate`
* `back-garbage-candidate`
* `dispute-ancestor`

## Integration test cases

To define integration tests create file
in the toml format as used with [zombienet][zombienet]
under `./integrationtests` describing the network to spawn and
also the `zndsl` file (with `.zndsl` extension ) using the format
defined in the [(DSL[(**D**omain **S**pecific **L**anguage)]) doc](https://paritytech.github.io/zombienet/cli/test-dsl-definition-spec.html).

## Usage

> Assumes you already gained permissiones, ping in element @javier:matrix.parity.io to get access.
> and you have cloned the [zombienet][zombienet] repo.

To launch a test case in the development cluster use (e.g. for the  ./node/malus/integrationtests/0001-dispute-valid-block.toml):

```sh
# declare the containers pulled in by zombie-net test definitions
export MALUS_IMAGE=docker.io/paritypr/malus:4131-ccd09bbf
export ZOMBIENET_INTEGRATION_TEST_IMAGE=docker.io/paritypr/synth-wave:4131-0.9.12-ccd09bbf-29a1ac18
export COL_IMAGE=docker.io/paritypr/colander:4131-ccd09bbf

# login chore, once, with the values as provided in the above guide
gcloud auth login
gcloud config set project "parity-zombienet"
gcloud container clusters get-credentials "parity-zombienet" --zone "europe-west3-b" --project parity-zombienet

# launching the actual test
cd zombienet
npm run build
node dist/cli.js test <path to polkadot repo>/node/malus/integrationtests/0001-dispute-valid-block.zndsl

# Access  logs (in google cloud storage)
gsutil ls gs://zombienet-logs/zombie-<namespace uniqueId>/logs/
```

This will also teardown the namespace after completion.

## Container Image Building Note

In order to build the container image you need to have the latest changes from
polkadot and substrate master branches.

```sh
pwd # run this from the current dir
podman build -t paritypr/malus:v1 -f Containerfile ../../..
```

[zombienet]: https://github.com/paritytech/zombienet
[gke]: (https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)
