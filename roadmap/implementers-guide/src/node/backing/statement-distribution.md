# Statement Distribution

The Statement Distribution Subsystem is responsible for distributing statements about seconded candidates between validators.

## Protocol

`PeerSet`: `Validation`

Input:

- `NetworkBridgeUpdate(update)`
- `StatementDistributionMessage`

Output:

- `NetworkBridge::SendMessage(PeerId, message)`
- `NetworkBridge::SendRequests(StatementFetchingV1)`
- `NetworkBridge::ReportPeer(PeerId, cost_or_benefit)`

## Functionality

Implemented as a gossip protocol. Handle updates to our view and peers' views. Neighbor packets are used to inform peers which chain heads we are interested in data for.

It is responsible for distributing signed statements that we have generated and forwarding them, and for detecting a variety of Validator misbehaviors for reporting to [Misbehavior Arbitration](../utility/misbehavior-arbitration.md). During the Backing stage of the inclusion pipeline, it's the main point of contact with peer nodes. On receiving a signed statement from a peer in the same backing group, assuming the peer receipt state machine is in an appropriate state, it sends the Candidate Receipt to the [Candidate Backing subsystem](candidate-backing.md) to handle the validator's statement. On receiving `StatementDistributionMessage::Share` we make sure to send messages to our backing group in addition to random other peers, to ensure a fast backing process and getting all statements quickly for distribution.

Track equivocating validators and stop accepting information from them. Establish a data-dependency order:

- In order to receive a `Seconded` message we have the corresponding chain head in our view
- In order to receive an `Valid` message we must have received the corresponding `Seconded` message.

And respect this data-dependency order from our peers by respecting their views. This subsystem is responsible for checking message signatures.

The Statement Distribution subsystem sends statements to peer nodes.

## Peer Receipt State Machine

There is a very simple state machine which governs which messages we are willing to receive from peers. Not depicted in the state machine: on initial receipt of any [`SignedFullStatement`](../../types/backing.md#signed-statement-type), validate that the provided signature does in fact sign the included data. Note that each individual parablock candidate gets its own instance of this state machine; it is perfectly legal to receive a `Valid(X)` before a `Seconded(Y)`, as long as a `Seconded(X)` has been received.

A: Initial State. Receive `SignedFullStatement(Statement::Second)`: extract `Statement`, forward to Candidate Backing, proceed to B. Receive any other `SignedFullStatement` variant: drop it.

B: Receive any `SignedFullStatement`: check signature and determine whether the statement is new to us. if new, forward to Candidate Backing and circulate to other peers. Receive `OverseerMessage::StopWork`: proceed to C.

C: Receive any message for this block: drop it.

For large statements (see below), we also keep track of the total received large
statements per peer and have a hard limit on that number for flood protection.
This is necessary as in the current code we only forward statements once we have
all the data, therefore flood protection for large statement is a bit more
subtle. This will become an obsolete problem once [off chain code
upgrades](https://github.com/paritytech/polkadot/issues/2979) are implemented.

## Peer Knowledge Tracking

The peer receipt state machine implies that for parsimony of network resources, we should model the knowledge of our peers, and help them out. For example, let's consider a case with peers A, B, and C, validators X and Y, and candidate M. A sends us a `Statement::Second(M)` signed by X. We've double-checked it, and it's valid. While we're checking it, we receive a copy of X's `Statement::Second(M)` from `B`, along with a `Statement::Valid(M)` signed by Y.

Our response to A is just the `Statement::Valid(M)` signed by Y. However, we haven't heard anything about this from C. Therefore, we send it everything we have: first a copy of X's `Statement::Second`, then Y's `Statement::Valid`.

This system implies a certain level of duplication of messages--we received X's `Statement::Second` from both our peers, and C may experience the same--but it minimizes the degree to which messages are simply dropped.

And respect this data-dependency order from our peers. This subsystem is responsible for checking message signatures.

No jobs. We follow view changes from the [`NetworkBridge`](../utility/network-bridge.md), which in turn is updated by the overseer.

## Equivocations and Flood Protection

An equivocation is a double-vote by a validator. The [Candidate Backing](candidate-backing.md) Subsystem is better-suited than this one to detect equivocations as it adds votes to quorum trackers.

At this level, we are primarily concerned about flood-protection, and to some extent, detecting equivocations is a part of that. In particular, we are interested in detecting equivocations of `Seconded` statements. Since every other statement is dependent on `Seconded` statements, ensuring that we only ever hold a bounded number of `Seconded` statements is sufficient for flood-protection.

The simple approach is to say that we only receive up to two `Seconded` statements per validator per chain head. However, the marginal cost of equivocation, conditional on having already equivocated, is close to 0, since a single double-vote offence is counted as all double-vote offences for a particular chain-head. Even if it were not, there is some amount of equivocations that can be done such that the marginal cost of issuing further equivocations is close to 0, as there would be an amount of equivocations necessary to be completely and totally obliterated by the slashing algorithm. We fear the validator with nothing left to lose.

With that in mind, this simple approach has a caveat worth digging deeper into.

First: We may be aware of two equivocated `Seconded` statements issued by a validator. A totally honest peer of ours can also be aware of one or two different `Seconded` statements issued by the same validator. And yet another peer may be aware of one or two _more_ `Seconded` statements. And so on. This interacts badly with pre-emptive sending logic. Upon sending a `Seconded` statement to a peer, we will want to pre-emptively follow up with all statements relative to that candidate. Waiting for acknowledgment introduces latency at every hop, so that is best avoided. What can happen is that upon receipt of the `Seconded` statement, the peer will discard it as it falls beyond the bound of 2 that it is allowed to store. It cannot store anything in memory about discarded candidates as that would introduce a DoS vector. Then, the peer would receive from us all of the statements pertaining to that candidate, which, from its perspective, would be undesired - they are data-dependent on the `Seconded` statement we sent them, but they have erased all record of that from their memory. Upon receiving a potential flood of undesired statements, this 100% honest peer may choose to disconnect from us. In this way, an adversary may be able to partition the network with careful distribution of equivocated `Seconded` statements.

The fix is to track, per-peer, the hashes of up to 4 candidates per validator (per relay-parent) that the peer is aware of. It is 4 because we may send them 2 and they may send us 2 different ones. We track the data that they are aware of as the union of things we have sent them and things they have sent us. If we receive a 1st or 2nd `Seconded` statement from a peer, we note it in the peer's known candidates even if we do disregard the data locally. And then, upon receipt of any data dependent on that statement, we do not reduce that peer's standing in our eyes, as the data was not undesired.

There is another caveat to the fix: we don't want to allow the peer to flood us because it has set things up in a way that it knows we will drop all of its traffic.
We also track how many statements we have received per peer, per candidate, and per chain-head. This is any statement concerning a particular candidate: `Seconded`, `Valid`, or `Invalid`. If we ever receive a statement from a peer which would push any of these counters beyond twice the amount of validators at the chain-head, we begin to lower the peer's standing and eventually disconnect. This bound is a massive overestimate and could be reduced to twice the number of validators in the corresponding validator group. It is worth noting that the goal at the time of writing is to ensure any finite bound on the amount of stored data, as any equivocation results in a large slash.

## Large statements

Seconded statements can become quite large on parachain runtime upgrades for
example. For this reason, there exists a `LargeStatement` constructor for the
`StatementDistributionMessage` wire message, which only contains light metadata
of a statement. The actual candidate data is not included. This message type is
used whenever a message is deemed large. The receiver of such a message needs to
request the actual payload via request/response by means of a
`StatementFetchingV1` request.

This is necessary as distribution of a large payload (mega bytes) via gossip
would make the network collapse and timely distribution of statements would no
longer be possible. By using request/response it is ensured that each peer only
transferes large data once. We only take good care to detect an overloaded
peer early and immediately move on to a different peer for fetching the data.
This mechanism should result in a good load distribution and therefore a rather
optimal distribution path.

With these optimizations, distribution of payloads in the size of up to 3 to 4
MB should work with Kusama validator specifications. For scaling up even more,
runtime upgrades and message passing should be done off chain at some point.

Flood protection considerations: For making DoS attacks slightly harder on this
subsystem, nodes will only respond to large statement requests, when they
previously notified that peer via gossip about that statement. So, it is not
possible to DoS nodes at scale, by requesting candidate data over and over
again.
