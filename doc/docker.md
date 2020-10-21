### The easiest way

The easiest/faster option to run Polkadot in docker is to use the latest
release images. These are small images that use the latest official release of
the polkadot binary, pulled from our package repository.

Let´s first check the version we have. The first time you run this command, the polkadot docker image will be downloaded. This takes a bit of time and bandwidth, be patient:

```bash
docker run --rm -it parity/polkadot:latest polkadot --version
```

You can also pass any argument/flag that polkadot supports:

```bash
docker run --rm -it parity/polkadot:latest polkadot --chain westend --name "PolkaDocker"
```

Once you are done experimenting and picking the best node name :) you can start polkadot as daemon, exposes the polkadot ports and mount a volume that will keep your blockchain data locally:

```bash
docker run -d -p 30333:30333 -p 9933:9933 -v /my/local/folder:/data parity/polkadot:latest polkadot --chain westend
```

Additionally if you want to have custom node name you can add the `--name "YourName"` at the end

```bash
docker run -d -p 30333:30333 -p 9933:9933 -v /my/local/folder:/data parity/polkadot:latest polkadot --chain westend --name "PolkaDocker"
```

```bash
docker run -d -p 30333:30333 -p 9933:9933 -v /my/local/folder:/data parity/polkadot:latest polkadot --rpc-external --chain westend
```

If you want to connect to rpc port 9933, then must add polkadot startup parameter: `--rpc-external`.

**Note:** The `--chain westend` argument is important and you need to add it to the command line. If you are running older node versions (pre 0.3) you don't need it.

### Limiting Resources

Chain syncing will utilise all available memory and CPU power your server has to offer, which can lead to crashing.

If running on a low resource VPS, use `--memory` and `--cpus` to limit the resources used. E.g. To allow a maximum of 512MB memory and 50% of 1 CPU, use `--cpus=".5" --memory="512m"`. Read more about limiting a container's resources [here](https://docs.docker.com/config/containers/resource_constraints).

Start a shell session with the daemon:

```bash
docker exec -it $(docker ps -q) bash;
```

Check the current version:

```bash
polkadot --version
```

### Build your own image

To get up and running with the smallest footprint on your system, you may use the Polkadot Docker image.
You can build it yourself (it takes a while...) in the shell session of the daemon:

```bash
cd docker
./build.sh
```

### Reporting issues

If you run into issues with polkadot when using docker, please run the following command
(replace the tag with the appropriate one if you do not use latest):

```bash
docker run --rm -it parity/polkadot:latest polkadot --version
```

This will show you the polkadot version as well as the git commit ref that was used to build your container.
Just paste that in the issue you create.
