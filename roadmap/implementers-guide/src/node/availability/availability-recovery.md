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
type ChunkResponse = Result<(PeerId, ErasureChunk), Unavailable>;

struct AwaitedChunk {
    issued_at: Instant,
    validator_index: ValidatorIndex,
    candidate_hash: CandidateHash,
    response: ResponseChannel<ChunkResponse>,
}

struct State {
    /// Each interaction is implemented as its own async task, and these handles are for communicating with them.
    interactions: Map<CandidateHash, InteractionHandle>,
    /// A recent block hash for which state should be available.
    live_block_hash: Hash,
    discovering_validators: Map<AuthorityDiscoveryId, Vec<AwaitedChunk>>,
    live_chunk_requests: Map<RequestId, (PeerId, AwaitedChunk)>,
    next_request_id: RequestId,
    connecting_validators: Stream<(AuthorityDiscoveryId, PeerId)>,

    /// interaction communication. This is cloned and given to interactions that are spun up.
    from_interaction_tx: Sender<FromInteraction>,
    /// receiver for messages from interactions.
    from_interaction_rx: Receiver<FromInteraction>,

    // An LRU cache of recently recovered data.
    availability_lru: LruCache<CandidateHash, Result<AvailableData, RecoveryError>>,
}

struct InteractionHandle {
    awaiting: Vec<ResponseChannel<Result<AvailableData, RecoveryError>>>,
}

struct Unavailable;
enum FromInteraction {
    // An interaction concluded.
    Concluded(CandidateHash, Result<AvailableData, RecoveryError>),
    // Make a request of a particular chunk from a particular validator.
    MakeRequest(
        AuthorityDiscoveryId, 
        CandidateHash, 
        ValidatorIndex, 
        ResponseChannel<ChunkResponse>,
    ),
    // Report a peer.
    ReportPeer(
        PeerId,
        Rep,
    ),
}

struct Interaction {
    to_state: Sender<FromInteraction>,
    validator_authority_keys: Vec<AuthorityId>,
    validators: Vec<ValidatorId>,
    // a random shuffling of the validators which indicates the order in which we connect to the validators and
    // request the chunk from them.
    shuffling: Vec<ValidatorIndex>, 
    // The number of pieces needed.
    threshold: usize, 
    candidate_hash: Hash,
    erasure_root: Hash,
    received_chunks: Map<ValidatorIndex, ErasureChunk>,
    requesting_chunks: FuturesUnordered<Receiver<ChunkResponse>>,
}
```

### Signal Handling

On `ActiveLeavesUpdate`, if `activated` is non-empty, set `state.live_block_hash` to the first block in `Activated`.

Ignore `BlockFinalized` signals.

On `Conclude`, shut down the subsystem.

#### `AvailabilityRecoveryMessage::RecoverAvailableData(receipt, session, response)`

1. Check the `availability_lru` for the candidate and return the data if so.
1. Check if there is already an interaction handle for the request. If so, add the response handle to it.
1. Otherwise, load the session info for the given session under the state of `live_block_hash`, and initiate an interaction with *launch_interaction*. Add an interaction handle to the state and add the response channel to it.
1. If the session info is not available, return `RecoveryError::Unavailable` on the response channel.

### From-interaction logic

#### `FromInteraction::Concluded`

1. Load the entry from the `interactions` map. It should always exist, if not for logic errors. Send the result to each member of `awaiting`.
1. Add the entry to the availability_lru.

#### `FromInteraction::MakeRequest(discovery_pub, candidate_hash, validator_index, response)`

1. Add an `AwaitedRequest` to the `discovering_validators` map under `discovery_pub`.
1. Issue a `NetworkBridgeMessage::ConnectToValidators`.
1. Add the stream of connected validator events to `state.connecting_validators`.

#### `FromInteraction::ReportPeer(peer, rep)`

1. Issue a `NetworkBridgeMessage::ReportPeer(peer, rep)`.

### Responding to network events.

#### On `connecting_validators` event:

1. If the validator exists under `discovering_validators`, remove the entry.
1. For each `AwaitedChunk` in the entry, issue a `AvailabilityRecoveryV1Message::RequestChunk(next_request_id, candidate_hash, validator_index)` and make an entry in the `live_chunk_requests` map.

#### On receiving `AvailabilityRecoveryV1::RequestChunk(r_id, candidate_hash, validator_index)`

1. Issue a `AvailabilityStore::QueryChunk(candidate-hash, validator_index, response)` message.
1. Whatever the result, issue a `AvailabilityRecoveryV1Message::Chunk(r_id, response)` message.

#### On receiving `AvailabilityRecoveryV1::Chunk(r_id, chunk)`

1. If there exists an entry under `r_id`, remove it. If there doesn't exist one, report the peer and return. If the peer in the entry doesn't match the sending peer, reinstate the entry, report the peer, and return.
1. Send the chunk response on the `awaited_chunk` for the interaction to handle.

### Interaction logic

#### `launch_interaction(session_index, session_info, candidate_receipt, candidate_hash)`

1. Compute the threshold from the session info. It should be `f + 1`, where `n = 3f + k`, where `k in {1, 2, 3}`, and `n` is the number of validators.
1. Set the various fields of `Interaction` based on the validator lists in `session_info`. Compute a random shuffling of the validator indices.
1. Set the `to_state` sender to be equal to a clone of `state.from_interaction_tx`.
1. Initialize `received_chunks` to an empty set, as well as `requesting_chunks`.

Launch the interaction as a background task running `interaction_loop(interaction)`.

#### `interaction_loop(interaction)`

```rust
// How many parallel requests to have going at once.
const N_PARALLEL: usize = 50;
```

Loop:
  * Poll for new updates from `requesting_chunks`. Check merkle proofs of any received chunks, and any failures should lead to issuance of a `FromInteraction::ReportPeer` message.
  * If `received_chunks` has more than `threshold` entries, attempt to recover the data. If that fails, or a re-encoding of it doesn't match the expected erasure root, break and issue a `FromInteraction::Concluded(RecoveryError::Invalid)`. Otherwise, issue a `FromInteraction::Concluded(Ok(()))`.
  * While there are fewer than `N_PARALLEL` entries in `requesting_chunks`,
    * Pop the next item from `shuffling`. If it's empty and `requesting_chunks` is empty, break and issue a `FromInteraction::Concluded(RecoveryError::Unavailable)`.
    * Initialize `(tx, rx)`.
    * Issue a `FromInteraction::MakeRequest(validator, candidate_hash, validator_index, tx)`.
    * Add `rx` to `requesting_chunks`.
