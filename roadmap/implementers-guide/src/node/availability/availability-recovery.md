# Availability Recovery

This subsystem is the inverse of the [Availability Distribution](availability-distribution.md) subsystem: validators will serve the availability chunks kept in the availability store to nodes who connect to them. And the subsystem will also implement the other side: the logic for nodes to connect to validators, request availability pieces, and reconstruct the `AvailableData`.

This version of the availability recovery subsystem is based off of direct connections to validators. In order to recover any given `AvailableData`, we must recover at least `f + 1` pieces from validators of the session. Thus, we will connect to and query randomly chosen validators until we have received `f + 1` pieces.

## Protocol

`PeerSet`: `Validation`

Input:

- NetworkBridgeUpdateV1(update)
- AvailabilityRecoveryMessage::RecoverAvailableData(candidate, session, backing_group, response)

Output:

- NetworkBridge::SendValidationMessage
- NetworkBridge::ReportPeer
- AvailabilityStore::QueryChunk

## Functionality

We hold a state which tracks the current recovery interactions we have live, as well as which request IDs correspond to which interactions. An interaction is a structure encapsulating all interaction with the network necessary to recover the available data.

```rust
struct State {
    /// Each interaction is implemented as its own remote async task, and these handles are remote
    /// for it.
    interactions: FuturesUnordered<InteractionHandle>,
    /// A multiplexer over receivers from live interactions.
    interaction_receivers: FuturesUnordered<ResponseReceiver<Concluded>>,
    /// A recent block hash for which state should be available.
    live_block_hash: Hash,
    // An LRU cache of recently recovered data.
    availability_lru: LruCache<CandidateHash, Result<AvailableData, RecoveryError>>,
}

/// This is a future, which concludes either when a response is received from the interaction,
/// or all the `awaiting` channels have closed.
struct InteractionHandle {
    candidate_hash: CandidateHash,
    interaction_response: RemoteHandle<Concluded>,
    awaiting: Vec<ResponseChannel<Result<AvailableData, RecoveryError>>>,
}

struct Unavailable;
struct Concluded(CandidateHash, Result<AvailableData, RecoveryError>);

struct InteractionParams {
    validator_authority_keys: Vec<AuthorityId>,
    validators: Vec<ValidatorId>,
    // The number of pieces needed.
    threshold: usize, 
    candidate_hash: Hash,
    erasure_root: Hash,
}

enum InteractionPhase {
    RequestFromBackers {
        // a random shuffling of the validators from the backing group which indicates the order
        // in which we connect to them and request the chunk.
        shuffled_backers: Vec<ValidatorIndex>,
    }
    RequestChunks {
        // a random shuffling of the validators which indicates the order in which we connect to the validators and
        // request the chunk from them.
        shuffling: Vec<ValidatorIndex>, 
        received_chunks: Map<ValidatorIndex, ErasureChunk>,
        requesting_chunks: FuturesUnordered<Receiver<ErasureChunkRequestResponse>>,
    }
}

struct Interaction {
    to_subsystems: SubsystemSender,
    params: InteractionParams,
    phase: InteractionPhase,
}
```

### Signal Handling

On `ActiveLeavesUpdate`, if `activated` is non-empty, set `state.live_block_hash` to the first block in `Activated`.

Ignore `BlockFinalized` signals.

On `Conclude`, shut down the subsystem.

#### `AvailabilityRecoveryMessage::RecoverAvailableData(receipt, session, Option<backing_group_index>, response)`

1. Check the `availability_lru` for the candidate and return the data if so.
1. Check if there is already an interaction handle for the request. If so, add the response handle to it.
1. Otherwise, load the session info for the given session under the state of `live_block_hash`, and initiate an interaction with *launch_interaction*. Add an interaction handle to the state and add the response channel to it.
1. If the session info is not available, return `RecoveryError::Unavailable` on the response channel.

### From-interaction logic

#### `FromInteraction::Concluded`

1. Load the entry from the `interactions` map. It should always exist, if not for logic errors. Send the result to each member of `awaiting`.
1. Add the entry to the availability_lru.

### Interaction logic

#### `launch_interaction(session_index, session_info, candidate_receipt, candidate_hash, Option<backing_group_index>)`

1. Compute the threshold from the session info. It should be `f + 1`, where `n = 3f + k`, where `k in {1, 2, 3}`, and `n` is the number of validators.
1. Set the various fields of `InteractionParams` based on the validator lists in `session_info` and information about the candidate.
1. If the `backing_group_index` is `Some`, start in the `RequestFromBackers` phase with a shuffling of the backing group validator indices and a `None` requesting value.
1. Otherwise, start in the `RequestChunks` phase with `received_chunks`,`requesting_chunks`, and `next_shuffling` all empty.
1. Set the `to_subsystems` sender to be equal to a clone of the `SubsystemContext`'s sender.
1. Initialize `received_chunks` to an empty set, as well as `requesting_chunks`.

Launch the interaction as a background task running `interaction_loop(interaction)`.

#### `interaction_loop(interaction) -> Result<AvailableData, RecoeryError>`

```rust
// How many parallel requests to have going at once.
const N_PARALLEL: usize = 50;
```

* Request `AvailabilityStoreMessage::QueryAvailableData`. If it exists, return that.
* If the phase is `InteractionPhase::RequestFromBackers`
  * Loop:
    * If the `requesting_pov` is `Some`, poll for updates on it. If it concludes, set `requesting_pov` to `None`. 
    * If the `requesting_pov` is `None`, take the next backer off the `shuffled_backers`.
        * If the backer is `Some`, issue a `NetworkBridgeMessage::Requests` with a network request for the `AvailableData` and wait for the response.
        * If it concludes with a `None` result, return to beginning. 
        * If it concludes with available data, attempt a re-encoding. 
            * If it has the correct erasure-root, break and issue a `Ok(available_data)`. 
            * If it has an incorrect erasure-root, return to beginning.
        * If the backer is `None`, set the phase to `InteractionPhase::RequestChunks` with a random shuffling of validators and empty `next_shuffling`, `received_chunks`, and `requesting_chunks` and break the loop.

* If the phase is `InteractionPhase::RequestChunks`:
  * Request `AvailabilityStoreMessage::QueryAllChunks`. For each chunk that exists, add it to `received_chunks` and remote the validator from `shuffling`.
  * Loop:
    * If `received_chunks + requesting_chunks + shuffling` lengths are less than the threshold, break and return `Err(Unavailable)`.
    * Poll for new updates from `requesting_chunks`. Check merkle proofs of any received chunks. If the request simply fails due to network issues, insert into the front of `shuffling` to be retried.
    * If `received_chunks` has more than `threshold` entries, attempt to recover the data. If that fails, or a re-encoding produces an incorrect erasure-root, break and issue a `Err(RecoveryError::Invalid)`. If correct, break and issue `Ok(available_data)`.
    * While there are fewer than `N_PARALLEL` entries in `requesting_chunks`,
      * Pop the next item from `shuffling`. If it's empty and `requesting_chunks` is empty, return `Err(RecoveryError::Unavailable)`.
      * Issue a `NetworkBridgeMessage::Requests` and wait for the response in `requesting_chunks`.
