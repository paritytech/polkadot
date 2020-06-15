# PoV Distribution

This subsystem is responsible for distributing PoV blocks. For now, unified with [Statement Distribution subsystem](/node/backing/statement-distribution.html).

## Protocol

Handle requests for PoV block by candidate hash and relay-parent.

## Functionality

Implemented as a gossip system, where `PoV`s are not accepted unless we know a `Seconded` message.

> TODO: this requires a lot of cross-contamination with statement distribution even if we don't implement this as a gossip system. In a point-to-point implementation, we still have to know _who to ask_, which means tracking who's submitted `Seconded`, `Valid`, or `Invalid` statements - by validator and by peer. One approach is to have the Statement gossip system to just send us this information and then we can separate the systems from the beginning instead of combining them
