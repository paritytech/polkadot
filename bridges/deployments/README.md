# Bridge Deployments

## Requirements
Make sure to install `docker` and `docker-compose` to be able to run and test bridge deployments. If
for whatever reason you can't or don't want to use Docker, you can find some scripts for running the
bridge [here](https://github.com/svyatonik/parity-bridges-common.test).

## Networks
One of the building blocks we use for our deployments are _networks_. A network is a collection of
homogenous blockchain nodes. We have Docker Compose files for each network that we want to bridge.
Each of the compose files found in the `./networks` folder is able to independently spin up a
network like so:

```bash
docker-compose -f ./networks/rialto.yml up
```

After running this command we would have a network of several nodes producing blocks.

## Bridges
A _bridge_ is a way for several _networks_ to connect to one another. Bridge deployments have their
own Docker Compose files which can be found in the `./bridges` folder. These Compose files typically
contain bridge relayers, which are services external to blockchain nodes, and other components such
as testing infrastructure, or user interfaces.

Unlike the network Compose files, these *cannot* be deployed on their own. They must be combined
with different networks.

In general, we can deploy the bridge using `docker-compose up` in the following way:

```bash
docker-compose -f <bridge>.yml \
               -f <network_1>.yml \
               -f <network_2>.yml \
               -f <monitoring>.yml up
```

If you want to see how the Compose commands are actually run, check out the source code of the
[`./run.sh`](./run.sh).

One thing worth noting is that we have a _monitoring_ Compose file. This adds support for Prometheus
and Grafana. We cover these in more details in the [Monitoring](#monitoring) section. At the moment
the monitoring Compose file is _not_ optional, and must be included for bridge deployments.

### Running and Updating Deployments
We currently support two bridge deployments
1. Ethereum PoA to Rialto Substrate
2. Rialto Substrate to Millau Substrate

These bridges can be deployed using our [`./run.sh`](./run.sh) script.

The first argument it takes is the name of the bridge you want to run. Right now we only support two
bridges: `poa-rialto` and `rialto-millau`.

```bash
./run.sh poa-rialto
```

If you add a second `update` argument to the script it will pull the latest images from Docker Hub
and restart the deployment.

```bash
./run.sh rialto-millau update
```

You can also bring down a deployment using the script with the `stop` argument.

```bash
./run.sh poa-rialto stop
```

### Adding Deployments
We need two main things when adding a new deployment. First, the new network which we want to
bridge. A compose file for the network should be added in the `/networks/` folder. Secondly we'll
need a new bridge Compose file in `./bridges/`. This should configure the bridge relayer nodes
correctly for the two networks, and add any additional components needed for the deployment. If you
want you can also add support in the `./run` script for the new deployment. While recommended it's
not strictly required.

## General Notes

Rialto authorities are named: `Alice`, `Bob`, `Charlie`, `Dave`, `Eve`.
Rialto-PoA authorities are named: `Arthur`, `Bertha`, `Carlos`.
Millau authorities are named: `Alice`, `Bob`, `Charlie`, `Dave`, `Eve`.

Both authorities and following accounts have enough funds (for test purposes) on corresponding Substrate chains:

- on Rialto: `Ferdie`, `George`, `Harry`.
- on Millau: `Ferdie`, `George`, `Harry`.

Names of accounts on Substrate (Rialto and Millau) chains may be prefixed with `//` and used as
seeds for the `sr25519` keys. This seed may also be used in the signer argument in Substrate
and PoA relays. Example:

```bash
./substrate-relay relay-headers rialto-to-millau \
	--rialto-host rialto-node-alice \
	--rialto-port 9944 \
	--millau-host millau-node-alice \
	--millau-port 9944 \
	--rialto-signer //Harry \
	--prometheus-host=0.0.0.0
```

Some accounts are used by bridge components. Using these accounts to sign other transactions
is not recommended, because this may lead to nonces conflict.

Following accounts are used when `poa-rialto` bridge is running:

- Rialto's `Alice` signs relay transactions with new Rialto-PoA headers;
- Rialto's `Bob` signs relay transactions with Rialto-PoA -> Rialto currency exchange proofs.
- Rialto-PoA's `Arthur`: signs relay transactions with new Rialto headers;
- Rialto-PoA's `Bertha`: signs currency exchange transactions.

Following accounts are used when `rialto-millau` bridge is running:

- Millau's `Charlie` signs relay transactions with new Rialto headers;
- Rialto's `Charlie` signs relay transactions with new Millau headers;
- Millau's `Dave` signs Millau transactions which contain messages for Rialto;
- Rialto's `Dave` signs Rialto transactions which contain messages for Millau;
- Millau's `Eve` signs relay transactions with message delivery confirmations from Rialto to Millau;
- Rialto's `Eve` signs relay transactions with messages from Millau to Rialto;
- Millau's `Ferdie` signs relay transactions with messages from Rialto to Millau;
- Rialto's `Ferdie` signs relay transactions with message delivery confirmations from Millau to Rialto.

### Docker Usage
When the network is running you can query logs from individual nodes using:

```bash
docker logs rialto_poa-node-bertha_1 -f
```

To kill all left over containers and start the network from scratch next time:
```bash
docker ps -a --format "{{.ID}}" | xargs docker rm # This removes all containers!
```

### Docker Compose Usage
If you're not familiar with how to use `docker-compose` here are some useful commands you'll need
when interacting with the bridge deployments:

```bash
docker-compose pull   # Get the latest images from the Docker Hub
docker-compose build  # This is going to build images
docker-compose up     # Start all the nodes
docker-compose up -d  # Start the nodes in detached mode.
docker-compose down   # Stop the network.
```

Note that for the you'll need to add the appropriate `-f` arguments that were mentioned in the
[Bridges](#bridges) section. You can read more about using multiple Compose files
[here](https://docs.docker.com/compose/extends/#multiple-compose-files). One thing worth noting is
that the _order_ the compose files are specified in matters. A different order will result in a
different configuration.

You can sanity check the final config like so:

```bash
docker-compose -f docker-compose.yml -f docker-compose.override.yml config > docker-compose.merged.yml
```

## Docker and Git Deployment
It is also possible to avoid using images from the Docker Hub and instead build
containers from Git. There are two ways to build the images this way.

### Git Repo
If you have cloned the bridges repo you can build local Docker images by running the following
command at the top level of the repo:

```bash
docker build . -t local/<project_you're_building> --build-arg=PROJECT=<project>
```

This will build a local image of a particular component with a tag of
`local/<project_you're_building>`. This tag can be used in Docker Compose files.

You can configure the build using using Docker
[build arguments](https://docs.docker.com/engine/reference/commandline/build/#set-build-time-variables---build-arg).
Here are the arguments currently supported:
  - `BRIDGE_REPO`: Git repository of the bridge node and relay code
  - `BRIDGE_HASH`: Commit hash within that repo (can also be a branch or tag)
  - `ETHEREUM_REPO`: Git repository of the OpenEthereum client
  - `ETHEREUM_HASH`: Commit hash within that repo (can also be a branch or tag)
  - `PROJECT`: Project to build withing bridges repo. Can be one of:
    - `rialto-bridge-node`
    - `millau-bridge-node`
    - `ethereum-poa-relay`
    - `substrate-relay`

### GitHub Actions
We have a nightly job which runs and publishes Docker images for the different nodes and relayers to
the [ParityTech Docker Hub](https://hub.docker.com/u/paritytech) organization. These images are used
for our ephemeral (temporary) test networks. Additionally, any time a tag in the form of `v*` is
pushed to GitHub the publishing job is run. This will build all the components (nodes, relayers) and
publish them.

With images built using either method, all you have to do to use them in a deployment is change the
`image` field in the existing Docker Compose files to point to the tag of the image you want to use.

### Monitoring
[Prometheus](https://prometheus.io/) is used by the bridge relay to monitor information such as system
resource use, and block data (e.g the best blocks it knows about). In order to visualize this data
a [Grafana](https://grafana.com/) dashboard can be used.

As part of the Rialto `docker-compose` setup we spin up a Prometheus server and Grafana dashboard. The
Prometheus server connects to the Prometheus data endpoint exposed by the bridge relay. The Grafana
dashboard uses the Prometheus server as its data source.

The default port for the bridge relay's Prometheus data is `9616`. The host and port can be
configured though the `--prometheus-host` and `--prometheus-port` flags. The Prometheus server's
dashboard can be accessed at `http://localhost:9090`. The Grafana dashboard can be accessed at
`http://localhost:3000`. Note that the default log-in credentials for Grafana are `admin:admin`.

### Environment Variables
Here is an example `.env` file which is used for production deployments and network updates. For
security reasons it is not kept as part of version control. When deploying a network this
file should be correctly populated and kept in the appropriate [`bridges`](`./bridges`) deployment
folder.

The `UI_SUBSTRATE_PROVIDER` variable lets you define the url of the Substrate node that the user
interface will connect to. `UI_ETHEREUM_PROVIDER` is used only as a guidance for users to connect
Metamask to the right Ethereum network. `UI_EXPECTED_ETHEREUM_NETWORK_ID`  is used by
the user interface as a fail safe to prevent users from connecting their Metamask extension to an
unexpected network.

```bash
GRAFANA_ADMIN_PASS=admin_pass
GRAFANA_SERVER_ROOT_URL=%(protocol)s://%(domain)s:%(http_port)s/
GRAFANA_SERVER_DOMAIN=server.domain.io
MATRIX_ACCESS_TOKEN="access-token"
WITH_PROXY=1 # Optional
UI_SUBSTRATE_PROVIDER=ws://localhost:9944
UI_ETHEREUM_PROVIDER=http://localhost:8545
UI_EXPECTED_ETHEREUM_NETWORK_ID=105
```

### UI

Use [wss://rialto.bridges.test-installations.parity.io/](https://polkadot.js.org/apps/)
as a custom endpoint for [https://polkadot.js.org/apps/](https://polkadot.js.org/apps/).

### Polkadot.js UI

To teach the UI decode our custom types used in the pallet, go to: `Settings -> Developer`
and import the [`./types.json`](./types.json)

## Scripts

The are some bash scripts in `scripts` folder that allow testing `Relay`
without running the entire network within docker. Use if needed for development.
