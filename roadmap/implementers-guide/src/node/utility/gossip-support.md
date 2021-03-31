# Gossip Support

The Gossip Support Subsystem is responsible for keeping track of session changes
and issuing a connection request to all validators in the current session if we are a validator.
The request will add all validators to the reserved PeerSet, meaning any validator in that set can
send us a message.

Gossiping subsystems will be notified when a new peer connects or disconnects by network bridge.
It is their responsibility to limit the amount of outgoing gossip messages.
At the moment we use enforce a cap of `max(sqrt(peer.len()), 25)` message recipients at a time in each gossiping subsystem.
