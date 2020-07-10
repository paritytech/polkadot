# Bitfield Distribution

Validators vote on the availability of a backed candidate by issuing signed bitfields, where each bit corresponds to a single candidate. These bitfields can be used to compactly determine which backed candidates are available or not based on a 2/3+ quorum.

## Protocol

`ProtocolId`: `b"bitd"`

Input:
[`BitfieldDistributionMessage`](../../types/overseer-protocol.md#bitfield-distribution-message) which are gossiped to all peers, no matter if validator or not.

Output:

- `NetworkBridge::RegisterEventProducer(ProtocolId)` in order to register ourselfs as an event provide for the protocol.
- `NetworkBridge::SendMessage([PeerId], ProtocolId, Bytes)`
- `NetworkBridge::ReportPeer(PeerId, cost_or_benefit)` improve or penalize the reputiation of peer based on the message we received relative to our state.
- `BitfieldDistributionMessage::DistributeBitfield(relay_parent, SignedAvailabilityBitfield)` gossip a verified incoming bitfield on to interested subsystems within this validator node.

## Functionality

This is implemented as a gossip system. Register a [network bridge](../utility/network-bridge.md) event producer on startup.

It is necessary to track peer connection, view change, and disconnection events, in order to maintain an index of which peers are interested in which relay parent bitfields.
Before gossiping incoming bitfields on, they must be checked to be signed by one of the validators
of the validator set relevant to the current relay parent.
Only accept bitfields relevant to our current view and only distribute bitfields to other peers when relevant to their most recent view.
Accept and distribute only one bitfield per validator.

When receiving a bitfield either from the network or from a `DistributeBitfield` message, forward it along to the block authorship (provisioning) subsystem for potential inclusion in a block.
