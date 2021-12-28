# Do I need this ?

Polkadot nodes collect and produce Prometheus metrics and logs. These include health, performance and debug 
information such as last finalized block, height of the chain, and many other deeper implementation details 
of the Polkadot/Substrate node subsystems. These are crucial pieces of information that one needs to successfully 
monitor the liveliness and performance of a network and its validators.

# How does it work ?

Just import the dashboard JSON files from this folder in your Grafana installation. All dashboards are grouped in 
folder percategory (like for example `parachains`). The files have been created by Grafana export functionality and
follow the data model specified [here](https://grafana.com/docs/grafana/latest/dashboards/json-model/).

We aim to keep the dashboards here in sync with the implementation, except dashboards for development and 
testing.

# Contributing

**Your contributions are most welcome!** 

Please make sure to follow the following design guidelines:
- Add a new entry in this file and describe the usecase and key metrics
- Ensure proper names and descriptions for dashboard panels and add relevant documentation when needed. 
This is very important as not all users have similar depth of understanding of the implementation 
- Have labels for axis
- All values have proper units of measurement
- A crisp and clear color scheme is used

# Prerequisites

Before you continue make sure you have Grafana set up, or otherwise follow this 
[guide](https://wiki.polkadot.network/docs/maintain-guides-how-to-monitor-your-node). 

You might also need to [setup Loki](https://grafana.com/go/webinar/loki-getting-started/).

# Alerting

Alerts are currently out of the scope of the dashboards, but their setup can be done manually or automated
(see [installing and configuring Alert Manager](https://wiki.polkadot.network/docs/maintain-guides-how-to-monitor-your-node#installing-and-configuring-alertmanager-optional))

# Dashboards

This section is a list of dashboards, their use case as well as the key metrics that are covered.

## Node Versions

Useful for monitoring versions and logs of validator nodes. Includes time series panels that 
track node warning and error log rates. These can be further investigated in Grafana Loki.

Requires Loki for log aggregation and querying.

[Dashboard JSON](general/kusama_deployment.json)

## Parachain Status

This dashboard allows you to see at a glance how fast are candidates approved, disputed and
finalized. It was originally designed for observing liveliness after parachain deployment in
 Kusama/Polkadot, but can be useful generally in production or testing.

It includes panels covering key subsystems of the parachain node side implementation:
- Backing
- PVF execution
- Approval voting
- Disputes coordinator
- Chain selection

It is important to note that this dashboard applies only for validator nodes. The prometheus 
queries assume the `instance` label value contains the string `validator` only for validator nodes. 

[Dashboard JSON](parachains/status.json)

### Key liveliness indicators
- **Relay chain finality lag**. How far behind finality is compared to the current best block. By design,
 GRANDPA never finalizes past last 2 blocks, so this value is always >=2 blocks.
- **Approval checking finality lag**. The distance (in blocks) between the chain head and the last block 
on which Approval voting is happening. The block is generally the highest approved ancestor of the head 
block and the metric is computed during relay chain selection.
- **Disputes finality lag**. How far behind the chain head is the last approved and non disputed block. 
This value is always higher than approval checking lag as it further restricts finality to only undisputed 
chains.
- **PVF preparation and execution time**. Each parachain has it's own PVF (parachain validation function): 
a wasm blob that is executed by validators during backing, approval checking and disputing. The PVF 
preparation time refers to the time it takes for the PVF wasm to be compiled. This step is done once and 
then result cached. PVF execution will use the resulting artifact to execute the PVF for a given candidate. 
PVFs are expected to have a limited execution time to ensure there is enough time left for the parachain 
block to be included in the relay block.
- **Time to recover and check candidate**. This is part of approval voting and covers the time it takes 
to recover the candidate block available data from other validators, check it (includes PVF execution time)
and issue statement or initiate dispute.
- **Assignment delay tranches**. Approval voting is designed such that validators assigned to check a specific 
candidate are split up into equal delay tranches (0.5 seconds each). All validators checks are ordered by the delay 
tranche index. Early tranches of validators have the opportunity to check the candidate first before later tranches 
that act as as backups in case of no shows.
