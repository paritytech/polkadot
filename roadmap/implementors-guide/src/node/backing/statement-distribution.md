# Statement Distribution

The Statement Distribution Subsystem is responsible for distributing statements about seconded candidates between validators.

## Protocol

`ProtocolId`: `b"stmd"`

Input:

- NetworkBridgeUpdate(update)

Output:

- NetworkBridge::RegisterEventProducer(`ProtocolId`)
- NetworkBridge::SendMessage(`[PeerId]`, `ProtocolId`, `Bytes`)
- NetworkBridge::ReportPeer(PeerId, cost_or_benefit)

## Functionality

Implemented as a gossip protocol. Register a network event producer on startup. Handle updates to our view and peers' views. Neighbor packets are used to inform peers which chain heads we are interested in data for.

Statement Distribution is the only backing subsystem which has any notion of peer nodes, who are any full nodes on the network. Validators will also act as peer nodes.

It is responsible for signing statements that we have generated and forwarding them, and for detecting a variety of Validator misbehaviors for reporting to [Misbehavior Arbitration](/node/utility/misbehavior-arbitration.html). During the Backing stage of the inclusion pipeline, it's the main point of contact with peer nodes, who distribute statements by validators. On receiving a signed statement from a peer, assuming the peer receipt state machine is in an appropriate state, it sends the Candidate Receipt to the [Candidate Backing subsystem](/node/backing/candidate-backing.html) to handle the validator's statement.

Track equivocating validators and stop accepting information from them. Forward double-vote proofs to the double-vote reporting system. Establish a data-dependency order:

- In order to receive a `Seconded` message we have the on corresponding chain head in our view
- In order to receive an `Invalid` or `Valid` message we must have received the corresponding `Seconded` message.

And respect this data-dependency order from our peers by respecting their views. This subsystem is responsible for checking message signatures.

The Statement Distribution subsystem sends statements to peer nodes and detects double-voting by validators. When validators conflict with each other or themselves, the Misbehavior Arbitration system is notified.

## Peer Receipt State Machine

There is a very simple state machine which governs which messages we are willing to receive from peers. Not depicted in the state machine: on initial receipt of any [`SignedStatement`](/type-definitions.html#signed-statement-type), validate that the provided signature does in fact sign the included data. Note that each individual parablock candidate gets its own instance of this state machine; it is perfectly legal to receive a `Valid(X)` before a `Seconded(Y)`, as long as a `Seconded(X)` has been received.

A: Initial State. Receive `SignedStatement(Statement::Second)`: extract `Statement`, forward to Candidate Backing, proceed to B. Receive any other `SignedStatement` variant: drop it.
B: Receive any `SignedStatement`: extract `Statement`, forward to Candidate Backing. Receive `OverseerMessage::StopWork`: proceed to C.
C: Receive any message for this block: drop it.

## Peer Knowledge Tracking

The peer receipt state machine implies that for parsimony of network resources, we should model the knowledge of our peers, and help them out. For example, let's consider a case with peers A, B, and C, validators X and Y, and candidate M. A sends us a `Statement::Second(M)` signed by X. We've double-checked it, and it's valid. While we're checking it, we receive a copy of X's `Statement::Second(M)` from `B`, along with a `Statement::Valid(M)` signed by Y.

Our response to A is just the `Statement::Valid(M)` signed by Y. However, we haven't heard anything about this from C. Therefore, we send it everything we have: first a copy of X's `Statement::Second`, then Y's `Statement::Valid`.

This system implies a certain level of duplication of messages--we received X's `Statement::Second` from both our peers, and C may experience the same--but it minimizes the degree to which messages are simply dropped.

And respect this data-dependency order from our peers. This subsystem is responsible for checking message signatures.

No jobs, `StartWork` and `StopWork` pulses are used to control neighbor packets and what we are currently accepting.
