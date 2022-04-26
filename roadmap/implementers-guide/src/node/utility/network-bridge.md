# Network Bridge

One of the main features of the overseer/subsystem duality is to avoid shared ownership of resources and to communicate via message-passing. However, implementing each networking subsystem as its own network protocol brings a fair share of challenges.

The most notable challenge is coordinating and eliminating race conditions of peer connection and disconnection events. If we have many network protocols that peers are supposed to be connected on, it is difficult to enforce that a peer is indeed connected on all of them or the order in which those protocols receive notifications that peers have connected. This becomes especially difficult when attempting to share peer state across protocols. All of the Parachain-Host's gossip protocols eliminate DoS with a data-dependency on current chain heads. However, it is inefficient and confusing to implement the logic for tracking our current chain heads as well as our peers' on each of those subsystems. Having one subsystem for tracking this shared state and distributing it to the others is an improvement in architecture and efficiency.

One other piece of shared state to track is peer reputation. When peers are found to have provided value or cost, we adjust their reputation accordingly.

So in short, this Subsystem acts as a bridge between an actual network component and a subsystem's protocol. The implementation of the underlying network component is beyond the scope of this module. We make certain assumptions about the network component:
  * The network allows registering of protocols and multiple versions of each protocol.
  * The network handles version negotiation of protocols with peers and only connects the peer on the highest version of the protocol.
  * Each protocol has its own peer-set, although there may be some overlap.
  * The network provides peer-set management utilities for discovering the peer-IDs of validators and a means of dialing peers with given IDs.


The network bridge makes use of the peer-set feature, but is not generic over peer-set. Instead, it exposes two peer-sets that event producers can attach to: `Validation` and `Collation`. More information can be found on the documentation of the [`NetworkBridgeMessage`][NBM].

## Protocol

Input: [`NetworkBridgeMessage`][NBM]


Output:
	- [`ApprovalDistributionMessage`][AppD]`::NetworkBridgeUpdate`
	- [`BitfieldDistributionMessage`][BitD]`::NetworkBridgeUpdate`
	- [`CollatorProtocolMessage`][CollP]`::NetworkBridgeUpdate`
	- [`StatementDistributionMessage`][StmtD]`::NetworkBridgeUpdate`

## Functionality

This network bridge sends messages of these types over the network.

```rust
enum WireMessage<M> {
	ProtocolMessage(M),
	ViewUpdate(View),
}
```

and instantiates this type twice, once using the [`ValidationProtocolV1`][VP1] message type, and once with the [`CollationProtocolV1`][CP1] message type.

```rust
type ValidationV1Message = WireMessage<ValidationProtocolV1>;
type CollationV1Message = WireMessage<CollationProtocolV1>;
```

### Startup

On startup, we register two protocols with the underlying network utility. One for validation and one for collation. We register only version 1 of each of these protocols.

### Main Loop

The bulk of the work done by this subsystem is in responding to network events, signals from the overseer, and messages from other subsystems.

Each network event is associated with a particular peer-set.

### Overseer Signal: `ActiveLeavesUpdate`

The `activated` and `deactivated` lists determine the evolution of our local view over time. A `ProtocolMessage::ViewUpdate` is issued to each connected peer on each peer-set, and a `NetworkBridgeEvent::OurViewChange` is issued to each event handler for each protocol.

We only send view updates if the node has indicated that it has finished major blockchain synchronization.

If we are connected to the same peer on both peer-sets, we will send the peer two view updates as a result.

### Overseer Signal: `BlockFinalized`

We update our view's `finalized_number` to the provided one and delay `ProtocolMessage::ViewUpdate` and `NetworkBridgeEvent::OurViewChange` till the next `ActiveLeavesUpdate`.

### Network Event: `PeerConnected`

Issue a `NetworkBridgeEvent::PeerConnected` for each [Event Handler](#event-handlers) of the peer-set and negotiated protocol version of the peer. Also issue a `NetworkBridgeEvent::PeerViewChange` and send the peer our current view, but only if the node has indicated that it has finished major blockchain synchronization. Otherwise, we only send the peer an empty view.

### Network Event: `PeerDisconnected`

Issue a `NetworkBridgeEvent::PeerDisconnected` for each [Event Handler](#event-handlers) of the peer-set and negotiated protocol version of the peer.

### Network Event: `ProtocolMessage`

Map the message onto the corresponding [Event Handler](#event-handlers) based on the peer-set this message was received on and dispatch via overseer.

### Network Event: `ViewUpdate`

- Check that the new view is valid and note it as the most recent view update of the peer on this peer-set.
- Map a `NetworkBridgeEvent::PeerViewChange` onto the corresponding [Event Handler](#event-handlers) based on the peer-set this message was received on and dispatch  via overseer.

### `ReportPeer`

- Adjust peer reputation according to cost or benefit provided

### `DisconnectPeer`

- Disconnect the peer from the peer-set requested, if connected.

### `SendValidationMessage` / `SendValidationMessages`

- Issue a corresponding `ProtocolMessage` to each listed peer on the validation peer-set.

### `SendCollationMessage` / `SendCollationMessages`

- Issue a corresponding `ProtocolMessage` to each listed peer on the collation peer-set.

### `ConnectToValidators`

- Determine the DHT keys to use for each validator based on the relay-chain state and Runtime API.
- Recover the Peer IDs of the validators from the DHT. There may be more than one peer ID per validator.
- Send all `(ValidatorId, PeerId)` pairs on the response channel.
- Feed all Peer IDs to peer set manager the underlying network provides.

### `NewGossipTopology`

- Map all `AuthorityDiscoveryId`s to `PeerId`s and issue a corresponding `NetworkBridgeUpdate`
  to all validation subsystems.

## Event Handlers

Network bridge event handlers are the intended recipients of particular network protocol messages. These are each a variant of a message to be sent via the overseer.

### Validation V1

* `ApprovalDistributionV1Message -> ApprovalDistributionMessage::NetworkBridgeUpdate`
* `BitfieldDistributionV1Message -> BitfieldDistributionMessage::NetworkBridgeUpdate`
* `StatementDistributionV1Message -> StatementDistributionMessage::NetworkBridgeUpdate`

### Collation V1

* `CollatorProtocolV1Message -> CollatorProtocolMessage::NetworkBridgeUpdate`

[NBM]: ../../types/overseer-protocol.md#network-bridge-message
[AppD]: ../../types/overseer-protocol.md#approval-distribution-message
[BitD]: ../../types/overseer-protocol.md#bitfield-distribution-message
[StmtD]: ../../types/overseer-protocol.md#statement-distribution-message
[CollP]: ../../types/overseer-protocol.md#collator-protocol-message

[VP1]: ../../types/network.md#validation-v1
[CP1]: ../../types/network.md#collation-v1
