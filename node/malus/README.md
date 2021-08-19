# malus

Create nemesis nodes with alternate, at best faulty, at worst intentionally destructive behavior traits.

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

There different behavior strains, pick one that you'd like to use with a build arg:

`$VARIANT` is one of:

* `dispute-ancestor`
* `second-garbage`

```sh
pwd # run this from the current dir
podman build --build-arg=$VARIANT -t paritypr/malus:v1 -f Containerfile ../../..
```

[gurke]: https://github.com/paritytech/gurke
[gke]: (https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)
