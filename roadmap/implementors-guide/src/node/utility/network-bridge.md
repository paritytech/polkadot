# Network Bridge

One of the main features of the overseer/subsystem duality is to avoid shared ownership of resources and to communicate via message-passing. However, implementing each networking subsystem as its own network protocol brings a fair share of challenges.

The most notable challenge is coordinating and eliminating race conditions of peer connection and disconnection events. If we have many network protocols that peers are supposed to be connected on, it is difficult to enforce that a peer is indeed connected on all of them or the order in which those protocols receive notifications that peers have connected. This becomes especially difficult when attempting to share peer state across protocols. All of the Parachain-Host's gossip protocols eliminate DoS with a data-dependency on current chain heads. However, it is inefficient and confusing to implement the logic for tracking our current chain heads as well as our peers' on each of those subsystems. Having one subsystem for tracking this shared state and distributing it to the others is an improvement in architecture and efficiency.

One other piece of shared state to track is peer reputation. When peers are found to have provided value or cost, we adjust their reputation accordingly.

So in short, this Subsystem acts as a bridge between an actual network component and a subsystem's protocol.

## Protocol

> REVIEW: I am designing this using dynamic dispatch based on a ProtocolId discriminant rather than doing static dispatch to specific subsystems based on a concrete network message type. The reason for this is that doing static dispatch might break the property that Subsystem implementations can be swapped out for others. So this is actually implementing a subprotocol multiplexer. Pierre tells me this is OK for our use-case ;). One caveat is that now all network traffic will also flow through the overseer, but this overhead is probably OK.

```rust
use sc-network::ObservedRole;

struct View(Vec<Hash>); // Up to `N` (5?) chain heads.

enum NetworkBridgeEvent {
 PeerConnected(PeerId, ObservedRole), // role is one of Full, Light, OurGuardedAuthority, OurSentry
 PeerDisconnected(PeerId),
 PeerMessage(PeerId, Bytes),
 PeerViewChange(PeerId, View), // guaranteed to come after peer connected event.
 OurViewChange(View),
}
```

Input:

- RegisterEventProducer(`ProtocolId`, `Fn(NetworkBridgeEvent) -> AllMessages`): call on startup.
- ReportPeer(PeerId, cost_or_benefit)
- SendMessage(`[PeerId]`, `ProtocolId`, Bytes): send a message to multiple peers.

## Functionality

Track a set of all Event Producers, each associated with a 4-byte protocol ID.
There are two types of network messages this sends and receives:

- ProtocolMessage(ProtocolId, Bytes)
- ViewUpdate(View)

`StartWork` and `StopWork` determine the computation of our local view. A `ViewUpdate` is issued to each connected peer, and a `NetworkBridgeUpdate::OurViewChange` is issued for each registered event producer.

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
