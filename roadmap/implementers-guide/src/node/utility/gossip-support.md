# Gossip Support

The Gossip Support Subsystem is responsible for keeping track of session changes
and issuing a connection request to all validators in the next, current and
a few past sessions if we are a validator in these sessions.
The request will add all validators to a reserved PeerSet, meaning we will not
reject a connection request from any validator in that set.

In addition to that, it creates a gossip overlay topology per session which
limits the amount of messages sent and received to be an order of sqrt of the
validators. Our neighbors in this graph will be forwarded to the network bridge
with the `NetworkBridgeMessage::NewGossipTopology` message.

See https://github.com/paritytech/polkadot/issues/3239 for more details.

The gossip topology is used by parachain distribution subsystems,
such as Bitfield Distribution, (small) Statement Distribution and
Approval Distribution to limit the amount of peers we send messages to
and handle view updates.
