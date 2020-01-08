# Parachains Roadmap
This is a roadmap for the core technology underlying Parachains - what protocols, APIs, and code paths need to be in place to fully instantiate a self-sufficient and secure parachain. We don't attempt to cover anything on what APIs a parachain toolkit might expose in order to make use of parachain features - only how those features are implemented and the low-level APIs that they expose to the validation function, if any.

## Categories
We will use these categories to delineate features:

*Runtime*: Runtime code for the Relay chain specifying consensus-critical state and updates that all full nodes must maintain or perform.

*Networking*: Protocols for nodes to speak to each other and transmit information across the network.

*Node*: State or updates that must be maintained or performed by some or all nodes off-chain. Often interfaces with networking components, and references runtime state.

---
## Sub-projects and features:
This section contains various sub-projects and the features that make them up.

### Infrastructure/API

#### *Peer Set Management*

Category: Networking

Validators assigned to a parachain need a way to discover and connect to collators in order to get fresh parachain blocks to validator.

Collators need to discover and connect to validators in order to submit parachain blocks,

Fishermen need to talk to validators and collators to fetch available data and circulate reports.

Some connections are long-lived, some are ephemeral (just for a single request)

#### Custom libp2p sub-protocols

Polkadot parachains involve many distinct networking protocols. Ideally, we'd be able to spawn each of these as a separate futures task which communicates via channel with other protocols or node code as necessary. This requires changes in Substrate and libp2p.

---
### Assignment

#### *Auctions*

Category: Runtime

Auctioning and registration of parachains. This is already implemented and follows the [Parachain Allocation — Research at W3F](https://research.web3.foundation/en/latest/polkadot/Parachain-Allocation.html) document.

#### *Parathread Auctions*

Category: Runtime

Parathreads are pay-as-you-go parachains.  This consists of an on-chain mechanism for resolving an auction by collators and ensuring that they author a block.

The node-side portion of parathreads is for collators to actually cast bids and to be configured for which conditions to cast bids under.

#### *Validator Assignment*

Category: Runtime

Assignment of validators to parachains. Validators are only assigned to parachains for a short period of time. Tweakable parameters include length of time assigned to each parachain and length of time in advance that the network is aware of validators' assignments.

---
### Agreement

#### *Attestation Circulation*

Category: Networking

A black-box networking component for circulating attestation messages (`Candidate`, `Valid`, `Invalid`) between validators of any given parachain to create a quorum on which blocks can be included.

#### *Availability Erasure-coding*

Category: Node, Networking

For each potential, considered parachain block, perform and erasure-coding of the PoV and outgoing messages of the block. Call the number of validators on the relay chain for the Relay-chain block this parachain block is being considered for inclusion in `n`. Erasure-code into `n` pieces, where any `f + 1` can recover (`f` being the maximum number of tolerated faulty nodes = ~ `n / 3`). The `i'th` validator stores the `i'th` piece of the coding and provides it to any who ask.

#### *PoV block fetching*

Category: Networking

A black-box networking component for validators or fishermen on a parachain to obtain the PoV block referenced by hash in an attestation, for the purpose of validating. When fetching "current" PoV blocks (close to the head of the chain, or relating to the block currently being built), this should be fast. When fetching "old" PoV blocks, it should be possible and fall back on recovering from the availability erasure-coding.

#### *Parathread Auction Voting*

Category: Node, Networking

How and when collators are configured to cast votes in parathread auctions.

#### *Collation Loop*

Category: Node, Networking

The main event loop of a collator node:
 1. new relay chain block B
 2. sync new parachain head w.r.t. B
 3. build new child of B
 4. submit to validators

---
### Cross-chain Messaging

https://hackmd.io/ILoQltEISP697oMYe4HbrA?view
https://github.com/paritytech/polkadot/issues/597

The biggest sub-project of the parachains roadmap - how messages are sent between parachains.

TODO: break down further.

---
### Fishing/Slashing

#### *Validity/Availability Report Handler*

Category: Runtime

In Polkadot, a bad parachain group can force inclusion of an invalid or unavailable parachain block. It is the job of fishermen to detect those blocks and report them to the runtime. This item is about the report handler

The W3F-research writeup on availability/validity provides a high-level view of the dispute resolution process: [Availability and Validity — Research at W3F](https://research.web3.foundation/en/latest/polkadot/Availability_and_Validity.html)

One of the main behaviors that is unimplemented and needs to be is the _rollback_ that occurs when the dispute resolution process concludes that an error has been made. When we mark a parachain block as having been invalid or unavailable, we need to roll back all parachains to a point from just before this state.  We would also need to roll back relay chain state, because there may have been messages from a parachain to a relay chain that now need to be rolled back.  The easiest thing to do would be to side-step that by putting a delay on upwards messages, but this would impact the UX of parachain participation in slot auctions, council votes, etc. considerably. Assuming we can't side-step this, we will have to find a way to roll back selected state of the relay chain.

#### *Double-vote Slash Handler*

Category: Runtime

In the attestation process, validators may submit only one `Candidate` message for a given relay chain block. If issuing a `Candidate` message on a parachain block, neither a `Valid` or `Invalid` vote cannot be issued on that parachain block, as the `Candidate` message is an implicit validity vote.  Otherwise, it is illegal to cast both a `Valid` and `Invalid` vote on a given parachain block.

Runtime handlers that take two conflicting votes as arguments and slash the offender are needed.

#### *Validity/Availability Fishing*

Category: Node

This code-path is also taken by validators who self-select based on VRF [Availability and Validity — Research at W3F](https://research.web3.foundation/en/latest/polkadot/Availability_and_Validity.html). Validators and fishermen will select parachain blocks to re-validate. In these steps:
* Attempt to recover the PoV block, falling back on the erasure-coding. If not available, issue report.
* Attempt to validate the PoV block. If invalid, issue report.

#### *Double-vote Fishing*

Category: Node

Nodes that observe a double-vote in the attestation process should submit a report to the chain to trigger slashing.

---

# Phases
This roadmap is divided up into phases, where each represents another set of deliverables or iteration on a black-box component with respect to the prior phase.

## Phase 0: MVP
The very first phase - this is parachains without slashing (full security) or cross-chain messaging. It is primarily a PoC that registration and validation are working correctly.

### Infrastructure/API:
  - Custom libp2p sub-protocols
  - Peer Set Management

### Assignment:
  - Auctions
  - Parathread Auctions
  - Validator Assignment

### Agreement:
  - Attestation Circulation (black box: gossip)
  - Availability Erasure-coding (black box: gossip)
  - PoV block fetching (black box: gossip)
  - Collation Loop

### Cross-chain Messaging:
  - TODO: probably just finalizing format to include egress bitfields.

## Phase 1: Fishing and Slashing

This phase marks advancement in the security of parachains. Once completed, parachains are a full-fledged cryptoeconomically secure rollup primitive. This phase also includes implementation work on XCMP, but does not enable it fully.

### Agreement
  - Availability Erasure-coding (black box: targeted distribution)
  - PoV block fetching (black box: targeted distribution and fetching)

### Fishing/Slashing
  - Validity/Availability Report Handler
  - Double-vote Slash Handler
  - Validity/Availability Fishing
  - Double-vote Fishing

## Phase 2: Messaging

This phase marks delivery of cross-chain messaging.

TODO: requires lots of XCMP stuff.
