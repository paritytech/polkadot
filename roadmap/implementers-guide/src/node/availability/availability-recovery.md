# Availability Recovery

> TODO: <https://github.com/paritytech/polkadot/issues/1597>

This subsystem is the inverse of the [Availability Distribution](availability-distribution.md) subsystem: validators will serve the availability chunks kept in the availability store to nodes who connect to them. And the subsystem will also implement the other side: the logic for nodes to connect to validators, request availability pieces, and reconstruct the `AvailableData`.

## Protocol

`PeerSet`: `Validation`

Input:

- NetworkBridgeUpdateV1(update)
- TODO: input message to request a fetch.

Output:

- NetworkBridge::SendValidationMessage
- NetworkBridge::ReportPeer
- AvailabilityStore::QueryChunk

## Functionality
