# Bitfield Distribution

Validators vote on the availability of a backed candidate by issuing signed bitfields, where each bit corresponds to a single candidate. These bitfields can be used to compactly determine which backed candidates are available or not based on a 2/3+ quorum.

## Protocol

`ProtocolId`: `b"bitd"`

Input: [`BitfieldDistributionMessage`](../../overseer-protocol.md#bitfield-distribution-message)
Output:

- `NetworkBridge::RegisterEventProducer(ProtocolId)`
- `NetworkBridge::SendMessage([PeerId], ProtocolId, Bytes)`
- `NetworkBridge::ReportPeer(PeerId, cost_or_benefit)`
- `BlockAuthorshipProvisioning::Bitfield(relay_parent, SignedAvailabilityBitfield)`

## Functionality

This is implemented as a gossip system. Register a [network bridge](../utility/network-bridge.html) event producer on startup and track peer connection, view change, and disconnection events. Only accept bitfields relevant to our current view and only distribute bitfields to other peers when relevant to their most recent view. Check bitfield signatures in this subsystem and accept and distribute only one bitfield per validator.

When receiving a bitfield either from the network or from a `DistributeBitfield` message, forward it along to the block authorship (provisioning) subsystem for potential inclusion in a block.
