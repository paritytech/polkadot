# Gossip Support

The Gossip Support Subsystem is responsible for keeping track of session changes
and issuing a connection request to all validators in the next, current and a few past sessions
if we are a validator in these sessions.
The request will add all validators to a reserved PeerSet, meaning we will not reject a connection request
from any validator in that set.

Gossiping subsystems will be notified when a new peer connects or disconnects by network bridge.
It is their responsibility to limit the amount of outgoing gossip messages.
At the moment we enforce a cap of `max(sqrt(peers.len()), 25)` message recipients at a time in each gossiping subsystem.

We also flip a coin with the same probability when handling peer view updates in the distribution subsystems.
Over time the probability of not handling a peer view update converges to zero, so it shouldn't be cause much trouble.
This should be considered as a temporary measure until we implement a more robust solution for gossiping.
