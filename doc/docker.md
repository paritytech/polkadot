# Using Containers

The following commands should work no matter if you use Docker or Podman. In general, Podman is recommended. All commands are "engine neutral" so you can use the container engine of your choice while still being able to copy/paste the commands below.

Let's start defining Podman as our engine:
```
ENGINE=podman
```

If you prefer to stick with Docker, use:
```
ENGINE=docker
```

## The easiest way

The easiest/faster option to run Polkadot in Docker is to use the latest release images. These are small images that use the latest official release of the Polkadot binary, pulled from our Debian package.

**_The following examples are running on westend chain and without SSL. They can be used to quick start and learn how Polkadot needs to be configured. Please find out how to secure your node, if you want to operate it on the internet. Do not expose RPC and WS ports, if they are not correctly configured._**

Let's first check the version we have. The first time you run this command, the Polkadot docker image will be downloaded. This takes a bit of time and bandwidth, be patient:

```bash
$ENGINE run --rm -it parity/polkadot:latest --version
```

You can also pass any argument/flag that Polkadot supports:

```bash
$ENGINE run --rm -it parity/polkadot:latest --chain westend --name "PolkaDocker"
```

## Examples

Once you are done experimenting and picking the best node name :) you can start Polkadot as daemon, exposes the Polkadot ports and mount a volume that will keep your blockchain data locally. Make sure that you set the ownership of your local directory to the Polkadot user that is used by the container.

Set user id 1000 and group id 1000, by running `chown 1000.1000 /my/local/folder -R` if you use a bind mount.

To start a Polkadot node on default rpc port 9933 and default p2p port 30333 use the following command. If you want to connect to rpc port 9933, then must add Polkadot startup parameter: `--rpc-external`.

```bash
$ENGINE run -d -p 30333:30333 -p 9933:9933 \
    -v /my/local/folder:/polkadot \
    parity/polkadot:latest \
    --chain westend --rpc-external --rpc-cors all \
    --name "PolkaDocker
```

If you also want to expose the webservice port 9944 use the following command:

```bash
$ENGINE run -d -p 30333:30333 -p 9933:9933 -p 9944:9944 \
    -v /my/local/folder:/polkadot \
    parity/polkadot:latest \
    --chain westend --ws-external --rpc-external --rpc-cors all --name "PolkaDocker"
```

## Using Docker compose

You can use the following docker-compose.yml file:

```bash
version: '2'

services:
  polkadot:
    container_name: polkadot
    image: parity/polkadot
    ports:
      - 30333:30333 # p2p port
      - 9933:9933 # rpc port
      - 9944:9944 # ws port
      - 9615:9615 # Prometheus port
    volumes:
      - /my/local/folder:/polkadot
    command: [
      "--name", "PolkaDocker",
      "--ws-external",
      "--rpc-external",
      "--prometheus-external",
      "--rpc-cors", "all"
    ]
```

With following `docker-compose.yml` you can set up a node and use polkadot-js-apps as the front end on port 80. After starting the node use a browser and enter your Docker host IP in the URL field: _<http://[YOUR_DOCKER_HOST_IP]>_

```bash
version: '2'

services:
  polkadot:
    container_name: polkadot
    image: parity/polkadot
    ports:
      - 30333:30333 # p2p port
      - 9933:9933 # rpc port
      - 9944:9944 # ws port
      - 9615:9615 # Prometheus port
    command: [
      "--name", "PolkaDocker",
      "--ws-external",
      "--rpc-external",
      "--prometheus-external",
      "--rpc-cors", "all"
    ]

  polkadotui:
    container_name: polkadotui
    image: jacogr/polkadot-js-apps
    environment:
      - WS_URL=ws://[YOUR_DOCKER_HOST_IP]:9944
    ports:
      - 80:80
```

## Limiting Resources

Chain syncing will utilize all available memory and CPU power your server has to offer, which can lead to crashing.

If running on a low resource VPS, use `--memory` and `--cpus` to limit the resources used. E.g. To allow a maximum of 512MB memory and 50% of 1 CPU, use `--cpus=".5" --memory="512m"`. Read more about limiting a container's resources [here](https://docs.docker.com/config/containers/resource_constraints).


## Build your own image

There are 3 options to build a polkadot container image:
- using the builder image
- using the injected "Debian" image
- using the generic injected image

### Builder image

To get up and running with the smallest footprint on your system, you may use an existing Polkadot Container image.

You may also build a polkadot container image yourself (it takes a while...) using the container specs `scripts/ci/dockerfiles/polkadot/polkadot_builder.Dockerfile`.

### Debian injected

The Debian injected image is how the official polkadot container image is produced. It relies on the Debian package that is published upon each release. The Debian injected image is usually available a few minutes after a new release is published.
It has the benefit of relying on the GPG signatures embedded in the Debian package.

### Generic injected

For simple testing purposes, the easiest option for polkadot and also random binaries, is to use the `binary_injected.Dockerfile` container spec. This option is less secure since the injected binary is not checked at all but it has the benefit to be simple. This option requires to already have a valid `polkadot` binary, compiled for Linux.

This binary is then simply copied inside the `parity/base-bin` image.

## Reporting issues

If you run into issues with Polkadot when using docker, please run the following command
(replace the tag with the appropriate one if you do not use latest):

```bash
$ENGINE run --rm -it parity/polkadot:latest --version
```

This will show you the Polkadot version as well as the git commit ref that was used to build your container.
You can now paste the version information in a [new issue](https://github.com/paritytech/polkadot/issues/new/choose).
