# Parity Bridges Common

This is a collection of components for building bridges.

These components include Substrate pallets for syncing headers, passing arbitrary messages, as well
as libraries for building relayers to provide cross-chain communication capabilities.

Three bridge nodes are also available. The nodes can be used to run test networks which bridge other
Substrate chains or Ethereum Proof-of-Authority chains.

🚧 The bridges are currently under construction - a hardhat is recommended beyond this point 🚧

## Contents
- [Installation](#installation)
- [High-Level Architecture](#high-level-architecture)
- [Project Layout](#project-layout)
- [Running the Bridge](#running-the-bridge)
- [How to send a message](#how-to-send-a-message)
- [Community](#community)

## Installation
To get up and running you need both stable and nightly Rust. Rust nightly is used to build the Web
Assembly (WASM) runtime for the node. You can configure the WASM support as so:

```
rustup install nightly
rustup target add wasm32-unknown-unknown --toolchain nightly
```

Once this is configured you can build and test the repo as follows:

```
git clone https://github.com/paritytech/parity-bridges-common.git
cd parity-bridges-common
cargo build --all
cargo test --all
```

If you need more information about setting up your development environment Substrate's
[Getting Started](https://substrate.dev/docs/en/knowledgebase/getting-started/) page is a good
resource.

## High-Level Architecture

This repo has support for bridging foreign chains together using a combination of Substrate pallets
and external processes called relayers. A bridge chain is one that is able to follow the consensus
of a foreign chain independently. For example, consider the case below where we want to bridge two
Substrate based chains.

```
+---------------+                 +---------------+
|               |                 |               |
|     Rialto    |                 |    Millau     |
|               |                 |               |
+-------+-------+                 +-------+-------+
        ^                                 ^
        |       +---------------+         |
        |       |               |         |
        +-----> | Bridge Relay  | <-------+
                |               |
                +---------------+
```

The Millau chain must be able to accept Rialto headers and verify their integrity. It does this by
using a runtime module designed to track GRANDPA finality. Since two blockchains can't interact
directly they need an external service, called a relayer, to communicate. The relayer will subscribe
to new Rialto headers via RPC and submit them to the Millau chain for verification.

Take a look at [Bridge High Level Documentation](./docs/high-level-overview.md) for more in-depth
description of the bridge interaction.

## Project Layout
Here's an overview of how the project is laid out. The main bits are the `node`, which is the actual
"blockchain", the `modules` which are used to build the blockchain's logic (a.k.a the runtime) and
the `relays` which are used to pass messages between chains.

```
├── bin             // Node and Runtime for the various Substrate chains
│  └── ...
├── deployments     // Useful tools for deploying test networks
│  └──  ...
├── diagrams        // Pretty pictures of the project architecture
│  └──  ...
├── modules         // Substrate Runtime Modules (a.k.a Pallets)
│  ├── ethereum     // Ethereum PoA Header Sync Module
│  ├── substrate    // Substrate Based Chain Header Sync Module
│  ├── message-lane // Cross Chain Message Passing
│  └──  ...
├── primitives      // Code shared between modules, runtimes, and relays
│  └──  ...
├── relays          // Application for sending headers and messages between chains
│  └──  ...
└── scripts         // Useful development and maintenence scripts
 ```

## Running the Bridge

To run the Bridge you need to be able to connect the bridge relay node to the RPC interface of nodes
on each side of the bridge (source and target chain).

There are 3 ways to run the bridge, described below:
 - building & running from source,
 - building or using Docker images for each individual component,
 - running a Docker Compose setup (recommended).

### Using the Source

First you'll need to build the bridge nodes and relay. This can be done as follows:

```bash
# In `parity-bridges-common` folder
cargo build -p rialto-bridge-node
cargo build -p millau-bridge-node
cargo build -p substrate-relay
```

### Running

To run a simple dev network you'll can use the scripts located in
[the `deployments/local-scripts` folder](./deployments/local-scripts). Since the relayer connects to
both Substrate chains it must be run last.

```bash
# In `parity-bridges-common` folder
./deployments/local-scripts/run-rialto-bridge-node.sh
./deployments/local-scripts/run-millau-bridge-node.sh
./deployments/local-scripts/relay-millau-to-rialto.sh
```

At this point you should see the relayer submitting headers from the Millau Substrate chain to the
Rialto Substrate chain.

### Local Docker Setup

To get up and running quickly you can use published Docker images for the bridge nodes and relayer.
The images are published on [Docker Hub](https://hub.docker.com/u/paritytech).

To run the dev network we first run the two bridge nodes:

```bash
docker run -p 30333:30333 -p 9933:9933 -p 9944:9944 \
           -it paritytech/rialto-bridge-node --dev --tmp \
           --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external

docker run -p 30334:30333 -p 9934:9933 -p 9945:9944 \
           -it paritytech/millau-bridge-node --dev --tmp \
           --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external
```

Notice that the `docker run` command will accept all the normal Substrate flags. For local
development you should at minimum run with the `--dev` flag or else no blocks will be produced.

Then we need to initialize and run the relayer:

```bash
docker run --network=host -it \
        paritytech/substrate-relay initialize-rialto-headers-bridge-in-millau \
        --millau-host localhost \
        --millau-port 9945 \
        --rialto-host localhost \
        --rialto-port 9944 \
        --millau-signer //Alice

docker run --network=host -it \
        paritytech/substrate-relay rialto-headers-to-millau \
        --millau-host localhost \
        --millau-port 9945 \
        --rialto-host localhost \
        --rialto-port 9944 \
        --millau-signer //Bob \
```

You should now see the relayer submitting headers from the Millau chain to the Rialto chain.

If you don't want to use the published Docker images you can build images yourself. You can do this
by running the following commands at the top level of the repository.

```bash
# In `parity-bridges-common` folder
docker build . -t local/rialto-bridge-node --build-arg PROJECT=rialto-bridge-node
docker build . -t local/millau-bridge-node --build-arg PROJECT=millau-bridge-node
docker build . -t local/substrate-relay --build-arg PROJECT=substrate-relay
```

_Note: Building the node images will take a long time, so make sure you have some coffee handy._

Once you have the images built you can use them in the previous commands by replacing
`paritytech/<component_name>` with `local/<component_name>` everywhere.

### Full Network Docker Compose Setup

For a more sophisticated deployment which includes bidirectional header sync, message passing,
monitoring dashboards, etc. see the [Deployments README](./deployments/README.md).

### How to send a message

A straightforward way to interact with and test the bridge is sending messages. This is explained
in the [send message](./docs/send-message.md) document.
## Community

Main hangout for the community is [Element](https://element.io/) (formerly Riot). Element is a chat
server like, for example, Discord. Most discussions around Polkadot and Substrate happen
in various Element "rooms" (channels). So, joining Element might be a good idea, anyway.

If you are interested in information exchange and development of Polkadot related bridges please
feel free to join the [Polkadot Bridges](https://app.element.io/#/room/#bridges:web3.foundation)
Element channel.

The [Substrate Technical](https://app.element.io/#/room/#substrate-technical:matrix.org) Element
channel is most suited for discussions regarding Substrate itself.

