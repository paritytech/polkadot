# malus

Create nemesis nodes with alternate, at best faulty, at worst intentionally destructive behavior traits.

The first argument determines the behavior strain. The currently supported are:

* `suggest-garbage-candidate`
* `back-garbage-candidate`
* `dispute-ancestor`

## Integration test cases

To define integration tests create file
in the toml format as used with [gurke][gurke]
under `./integrationtests` with either extension
`.toml` or `.toml.tera` depending on if you use
tera based templating.

> For the time being non-templated variants should be preferred!
## Usage

> Assumes you already gained permissiones, followed the [GKE access guide][gke],
> and you installed [gurke][gurke].

To launch a test case in the development cluster use (e.g. for the  ./node/malus/integrationtests/0001-dispute-valid-block.toml):

```sh
# declare the containers pulled in by gurke test definitions
export SYNTHIMAGE=paritypr/synth-wave:3639-0.9.9-7edc6602-ed5fb773
export COLIMAGE=paritypr/colander:3639-7edc6602
export MALUSIMAGE=paritypr/malus:3639-7edc6602
export SCRIPTSIMAGE=paritytech/simnet:v9

# login chore, once, with the values as provided in the above guide
gcloud auth login
gcloud config set project "parity-simnet"
gcloud container clusters get-credentials "parity-simnet-devtest" --zone "europe-west3-b"

# launching the actual test
gurke run -c ./node/malus/integrationtests/0001-dispute-valid-block.toml -n parity-simnet-devtest ./node/malus/integrationtests/0001-dispute-valid-block.feature

# Access individual logs
kubectl -n parity-simnet-devtest logs mal
```

This will also teardown the cluster after completion.

## Container Image Building Note

In order to build the container image you need to have the latest changes from
polkadot and substrate master branches.

```sh
pwd # run this from the current dir
podman build -t paritypr/malus:v1 -f Containerfile ../../..
```

[gurke]: https://github.com/paritytech/gurke
[gke]: (https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)
