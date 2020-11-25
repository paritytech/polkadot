# Availability Recovery

> TODO: <https://github.com/paritytech/polkadot/issues/1597>

This subsystem is the inverse of the [Availability Distribution](availability-distribution.md) subsystem: validators will serve the availability chunks kept in the availability store to nodes who connect to them. And the subsystem will also implement the other side: the logic for nodes to connect to validators, request availability pieces, and reconstruct the `AvailableData`.

This version of the availability recovery subsystem is based off of direct connections to validators. In order to recover any given `AvailableData`, we must recover at least `f + 1` pieces from validators of the session. Thus, we will connect to and query randomly chosen validators until we have received `f + 1` pieces.

## Protocol

`PeerSet`: `Validation`

Input:

- NetworkBridgeUpdateV1(update)
- AvailabilityRecoveryMessage::RecoverAvailableData(candidate, session, response)

Output:

- NetworkBridge::SendValidationMessage
- NetworkBridge::ReportPeer
- AvailabilityStore::QueryChunk

## Functionality
