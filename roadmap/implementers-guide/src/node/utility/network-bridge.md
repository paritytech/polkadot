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
Output: Varying, based on registered event producers.

## Functionality

These are the types of network messages this sends and receives:

- `ProtocolMessage(NetworkCapability, Bytes)`
- `ViewUpdate(View)`

Each network message is associated with a particular peer-set. If we are connected to the same peer on both peer-sets, we will receive two `ViewUpdate`s from them every time they change their view.

`ActiveLeavesUpdate`'s `activated` and `deactivated` lists determine the evolution of our local view over time. A `ViewUpdate` is issued to each connected peer after each update, and a `NetworkBridgeUpdate::OurViewChange` is issued for each registered event producer.

On `ProtocolMessage` arrival:

- If the network capability and peer-set map to a message type, produce the message from the `NetworkBridgeEvent::PeerMessage(sender, bytes)`, otherwise ignore and reduce peer reputation slightly
- dispatch message via overseer.

On `ViewUpdate` arrival:

- Do validity checks and note the most recent view update of the peer.
- For each capability supported by the intersection of the peer's supported versions and our supported versions, dispatch the result of a `NetworkBridgeEvent::PeerViewChange(view)` via overseer.

On `ReportPeer` message:

- Adjust peer reputation according to cost or benefit provided

On `SendMessage` message:

- Issue a corresponding `ProtocolMessage` to each listed peer with given network capability and bytes.

[NBM]: ../../types/overseer-protocol.md#network-bridge-message

On `ConnectToValidators` message:

- Determine the DHT keys to use for each validator based on the relay-chain state and Runtime API.
- Recover the Peer IDs of the validators from the DHT. There may be more than one peer ID per validator.
- Accumulate all `(ValidatorId, PeerId)` pairs and send on the response channel.
- Feed all Peer IDs to peer set manager the underlying network provides. At the time of writing, this is beyond the scope of the guide.

## Version-to-capability Mappings

This section contains the mappings for different protocol versions to capabilities and the message types that should be transmitted via the overseer as a result.

### Validation V1

* `"stmd"`
* `"povd"`
* `"avad"`
* `"bitd"`

### Collation V1

* `"colp"`

## Capability to Message Type Mappings

* `"stmd" -> StatementDistributionMessage::NetworkBridgeUpdate`
* `"povd" -> PoVDistributionMessage::NetworkBridgeUpdate`
* `"avad" -> AvailabilityDistributionMessage::NetworkBridgeUpdate`
* `"bitd" -> BitfieldDistributionMessage::NetworkBridgeUpdate`
* `"colp" -> CollatorProtocolMessage::NetworkBridgeUpdate`
