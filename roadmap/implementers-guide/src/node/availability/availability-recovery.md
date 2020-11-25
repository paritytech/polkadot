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

We hold a state which tracks the current recovery interactions we have live, as well as which request IDs correspond to which interactions. An interaction is a structure encapsulating all interaction with the network necessary to recover the available data.

```rust
struct State {
    /// The interaction handle may just be the interaction itself, but it might be useful to implement each interaction
    /// as its own `Future`, running in its own task, in which case the handle will be a channel to communicate with
    /// the interaction.
    interactions: Map<CandidateHash, InteractionHandle>,
    live_requests: Map<RequestId, (PeerId, CandidateHash)>,
    next_request_id: RequestId,

    // An LRU cache of recently recovered data.
    availability_lru: LruCache<CandidateHash, AvailableData>,
}
```

```rust
struct Interaction {
    responses: ResponseChannel<Option<AvailableData>>,
    validator_authority_keys: Vec<AuthorityId>,
    validators: Vec<ValidatorId>,
    // a random shuffling of the validators which indicates the order in which we connect to the validators and
    // request the chunk from them.
    shuffling: Vec<ValidatorIndex>, 
    // The number of pieces needed.
    threshold: usize, 
    received_chunks: Map<ValidatorIndex, ErasureChunk>,
    requesting_chunks: Set<ValidatorIndex>,
}
```