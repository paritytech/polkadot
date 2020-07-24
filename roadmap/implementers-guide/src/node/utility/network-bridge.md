# Network Bridge

One of the main features of the overseer/subsystem duality is to avoid shared ownership of resources and to communicate via message-passing. However, implementing each networking subsystem as its own network protocol brings a fair share of challenges.

The most notable challenge is coordinating and eliminating race conditions of peer connection and disconnection events. If we have many network protocols that peers are supposed to be connected on, it is difficult to enforce that a peer is indeed connected on all of them or the order in which those protocols receive notifications that peers have connected. This becomes especially difficult when attempting to share peer state across protocols. All of the Parachain-Host's gossip protocols eliminate DoS with a data-dependency on current chain heads. However, it is inefficient and confusing to implement the logic for tracking our current chain heads as well as our peers' on each of those subsystems. Having one subsystem for tracking this shared state and distributing it to the others is an improvement in architecture and efficiency.

One other piece of shared state to track is peer reputation. When peers are found to have provided value or cost, we adjust their reputation accordingly.

So in short, this Subsystem acts as a bridge between an actual network component and a subsystem's protocol.

The other component of the network bridge is which peer-set to use. Different peer-sets can be connected for different purposes. The network bridge is not generic over peer-set, but instead exposes two peer-sets that event producers can attach to: `Validation` and `Collation`. More information can be found on the documentation of the [`NetworkBridgeMessage`][NBM].

## Protocol

Input: [`NetworkBridgeMessage`][NBM]
Output: Varying, based on registered event producers.

## Functionality

Track a set of all Event Producers, each associated with a 4-byte protocol ID and the `PeerSet` it is associated on.

There are two types of network messages this sends and receives:

- ProtocolMessage(ProtocolId, Bytes)
- ViewUpdate(View)

Each of these network messages is associated with a particular peer-set. If we are connected to the same peer on both peer-sets, we will receive two `ViewUpdate`s from them every time they change their view.

`ActiveLeavesUpdate`'s `activated` and `deactivated` lists determine the evolution of our local view over time. A `ViewUpdate` is issued to each connected peer after each update, and a `NetworkBridgeUpdate::OurViewChange` is issued for each registered event producer.

On `RegisterEventProducer`:

- Add the event producer to the set of event producers. If there is a competing entry, ignore the request.

On `ProtocolMessage` arrival:

- If the protocol ID matches an event producer, produce the message from the `NetworkBridgeEvent::PeerMessage(sender, bytes)`, otherwise ignore and reduce peer reputation slightly
- dispatch message via overseer.

On `ViewUpdate` arrival:

- Do validity checks and note the most recent view update of the peer.
- For each event producer, dispatch the result of a `NetworkBridgeEvent::PeerViewChange(view)` via overseer.

On `ReportPeer` message:

- Adjust peer reputation according to cost or benefit provided

On `SendMessage` message:

- Issue a corresponding `ProtocolMessage` to each listed peer with given protocol ID and bytes.

[NBM]: ../../types/overseer-protocol.md#network-bridge-message

On `ConnectToValidators` message:

- Determine the DHT keys to use for each validator based on the relay-chain state and Runtime API.
- Recover the Peer IDs of the validators from the DHT. There may be more than one peer ID per validator.
- Accumulate all `(ValidatorId, PeerId)` pairs and send on the response channel.
- Feed all Peer IDs to the discovery utility the underlying network provides.
