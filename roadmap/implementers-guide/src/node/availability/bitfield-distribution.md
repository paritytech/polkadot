# Bitfield Distribution

Validators vote on the availability of a backed candidate by issuing signed bitfields, where each bit corresponds to a single candidate. These bitfields can be used to compactly determine which backed candidates are available or not based on a 2/3+ quorum.

## Protocol

`PeerSet`: `Validation`

Input:
[`BitfieldDistributionMessage`](../../types/overseer-protocol.md#bitfield-distribution-message) which are gossiped to all peers, no matter if validator or not.

Output:

- `NetworkBridge::SendValidationMessage([PeerId], message)` gossip a verified incoming bitfield on to interested subsystems within this validator node.
- `NetworkBridge::ReportPeer(PeerId, cost_or_benefit)` improve or penalize the reputation of peers based on the messages that are received relative to the current view.
- `ProvisionerMessage::ProvisionableData(ProvisionableData::Bitfield(relay_parent, SignedAvailabilityBitfield))` pass
  on the bitfield to the other submodules via the overseer.

## Functionality

This is implemented as a gossip system.

It is necessary to track peer connection, view change, and disconnection events, in order to maintain an index of which peers are interested in which relay parent bitfields.


Before gossiping incoming bitfields, they must be checked to be signed by one of the validators
of the validator set relevant to the current relay parent.
Only accept bitfields relevant to our current view and only distribute bitfields to other peers when relevant to their most recent view.
Accept and distribute only one bitfield per validator.


When receiving a bitfield either from the network or from a `DistributeBitfield` message, forward it along to the block authorship (provisioning) subsystem for potential inclusion in a block.

Peers connecting after a set of valid bitfield gossip messages was received, those messages must be cached and sent upon connection of new peers or re-connecting peers.
