# Node Architecture

## Design Goals

* Modularity: Components of the system should be as self-contained as possible. Communication boundaries between components should be well-defined and mockable. This is key to creating testable, easily reviewable code.
* Minimizing side effects: Components of the system should aim to minimize side effects and to communicate with other components via message-passing.
* Operational Safety: The software will be managing signing keys where conflicting messages can lead to large amounts of value to be slashed. Care should be taken to ensure that no messages are signed incorrectly or in conflict with each other.

The architecture of the node-side behavior aims to embody the Rust principles of ownership and message-passing to create clean, isolatable code. Each resource should have a single owner, with minimal sharing where unavoidable.

Many operations that need to be carried out involve the network, which is asynchronous. This asynchrony affects all core subsystems that rely on the network as well. The approach of hierarchical state machines is well-suited to this kind of environment.

We introduce

## Components

The node architecture consists of the following components:
  * The Overseer (and subsystems): A hierarchy of state machines where an overseer supervises subsystems. Subsystems can contain their own internal hierarchy of jobs. This is elaborated on in the next section on Subsystems.
  * A block proposer: Logic triggered by the consensus algorithm of the chain when the node should author a block.
  * A GRANDPA voting rule: A strategy for selecting chains to vote on in the GRANDPA algorithm to ensure that only valid parachain candidates appear in finalized relay-chain blocks.

## Assumptions

The Node-side code comes with a set of assumptions that we build upon. These assumptions encompass most of the fundamental blockchain functionality.

We assume the following constraints regarding provided basic functionality:
  * The underlying **consensus** algorithm, whether it is BABE or SASSAFRAS is implemented.
  * There is a **chain synchronization** protocol which will search for and download the longest available chains at all times.
  * The **state** of all blocks at the head of the chain is available. There may be **state pruning** such that state of the last `k` blocks behind the last finalized block are available, as well as the state of all their descendants. This assumption implies that the state of all active leaves and their last `k` ancestors are all available. The underlying implementation is expected to support `k` of a few hundred blocks, but we reduce this to a very conservative `k=5` for our purposes.
  * There is an underlying **networking** framework which provides **peer discovery** services which will provide us with peers and will not create "loopback" connections to our own node. The number of peers we will have is assumed to be bounded at 1000.
  * There is a **transaction pool** and a **transaction propagation** mechanism which maintains a set of current transactions and distributes to connected peers. Current transactions are those which are not outdated relative to some "best" fork of the chain, which is part of the active heads, and have not been included in the best fork.
