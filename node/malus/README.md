# malus

Create nemesis nodes with alternate, at best fault, at worst intentionally destructive behavior traits.

## Integration test cases

To define integration tests create file
in the toml format as used with [gurke][gurke]
under `./integrationtests` with either extension
`.toml` or `.toml.tera` depending if you use
tera based templating.

## Container Image Building Note

In order to build the container image you need to have latest changes from
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
