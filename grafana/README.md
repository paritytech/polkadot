## Do I need this ?

Polkadot nodes collect and prouce Prometheus metrics and logs. These include health, performance and debug 
information such as last finalized block, height of the chain, CPU usage, and so one. However, most of these
cover deeper implementation details in multiple subsystems. These are crucial pieces of information that one 
needs to succesfully monitor the liveliness and performance of a network and its validators.

## How does it work ?

Just import the dashboard JSON files from this folder in Grafana. All dashboards are grouped in folder per
category (like for example `parachains`). The files have been created by Grafana export functionality and
follow the data model specified [here](https://grafana.com/docs/grafana/latest/dashboards/json-model/).

We aim to keep the dashboards here in sync with the implementation, except dashboards for development and 
testing.

## Contributing

**Your contributions are most welcome!** 

Please make sure to follow the following design guidelines:
- Provide a clear usecase and make
- Ensure proper names and descriptions for dashboard panels and relevant documentation when needed. 
This is very important as not all users have similar depth of understanding of the implementation 
- Have labels for axis
- All values have proper units of measurement
- A crisp and clear color scheme is used

## Prerequisites

Before you continue make sure you have Grafana set up, or otherwise follow this 
[guide](https://wiki.polkadot.network/docs/maintain-guides-how-to-monitor-your-node). 

## Alerting

Alerts are currently out of the scope of the dashboards, but their setup can be done manually or automated
(see [installing and configuring AlertManager](https://wiki.polkadot.network/docs/maintain-guides-how-to-monitor-your-node#installing-and-configuring-alertmanager-optional))

## Dashboards

This section describes the dashboards and their use case as well as the KPIs that are covered.

### Dashboard: Node versions

Useful for monitoring versions and logs of validator nodes. Provides charts for warning and error 
rates per target subsystem.

Requires Loki for log aggregation and querying.

### Dashboiard: Parachain liveliness

This dashboard allows you to see at a glance how fast are candidates approved, disputed and
finalized. 
It includes panels covering key subsystems of the parachain node side implementation:
- Backing
- PVF execution
- Appproval voting
- Disputes coordinator
- Chain selection

Key indicators:
- **Relay chain finality lag**. How far behind finality is compared to the current best block. By design, GRANDPA never finalizes past last 2 blocks, so this value is always >=2 blocks.
- **Approval checking finality lag**. How far behind the chain head is the block on which Approval voting is happening. The block is generally the highest approved ancestor of the head block and the metric is computed during relay chain selection.
- **Disputes finality lag**. How far behind the chain head is the last non disputed block. This value is always higher than approval checking lag as it further restricts finality to only undisputed chains.