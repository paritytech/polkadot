# Dispute Coordinator

This is the central subsystem of the node-side components which participate in disputes. This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions. Votes older than this session window are pruned.

This subsystem will be the point which produce dispute votes, eiuther positive or negative, based on locally-observed validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from another node, this will trigger the [dispute participation](dispute-participation.md) subsystem to recover and validate the block and call back to this subsystem.

## Database Schema

We use an underlying Key-Value database where we assume we have the following operations available:
  * `write(key, value)`
  * `read(key) -> Option<value>`
  * `iter_with_prefix(prefix) -> Iterator<(key, value)>` - gives all keys and values in lexicographical order where the key starts with `prefix`.

We use this database to encode the following schema:

```rust
(SessionIndex, "candidate-votes", CandidateHash) -> Option<CandidateVotes>
"active-disputes" -> ActiveDisputes
"earliest-session" -> Option<SessionIndex>
```

The meta information that we track per-candidate is defined as the `CandidateVotes` struct.
This draws on the [dispute statement types][DisputeTypes]

```rust
struct CandidateVotes {
    // The receipt of the candidate itself.
    candidate_receipt: CandidateReceipt,
    // Sorted by validator index.
    valid: Vec<(ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
    // Sorted by validator index.
    invalid: Vec<(InvalidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
}

struct ActiveDisputes {
    // sorted by session index and then by candidate hash.
    disputed: Vec<(SessionIndex, CandidateHash)>,
}
```

## Protocol

Input: [`DisputeCoordinatorMessage`][DisputeCoordinatorMessage]

Output:
  - [`RuntimeApiMessage`][RuntimeApiMessage]
  - [`DisputeParticipationMessage`][DisputeParticipationMessage]

## Functionality

This assumes a constant `DISPUTE_WINDOW: SessionIndex`. This should correspond to at least 1 day.

Ephemeral in-memory state:

```rust
struct State {
    keystore: KeyStore,
    // An in-memory overlay used as a write-cache.
    overlay: Map<(SessionIndex, CandidateReceipt), CandidateVotes>,
    highest_session: SessionIndex,
}
```

### On `OverseerSignal::ActiveLeavesUpdate`

For each leaf in the leaves update:
  * Fetch the session index for the child of the block with a [`RuntimeApiMessage::SessionIndexForChild`][RuntimeApiMessage].
  * If the session index is higher than `state.highest_session`:
    * update `state.highest_session`
    * remove everything with session index less than `state.highest_session - DISPUTE_WINDOW` from the overlay and from the `"active-disputes"` in the DB.
    * Use `iter_with_prefix` to remove everything from `"earliest-session"` up to `state.highest_session - DISPUTE_WINDOW` from the DB under `"candidate-votes"`.
    * Update `"earliest-session"` to be equal to `state.highest_session - DISPUTE_WINDOW`.
  * For each new block, explicitly or implicitly, under the new leaf, scan for a dispute digest which indicates a rollback. If a rollback is detected, use the ChainApi subsystem to blacklist the chain.

### On `OverseerSignal::Conclude`

Flush the overlay to DB and conclude.

### On `OverseerSignal::BlockFinalized`

Do nothing.

### On `DisputeCoordinatorMessage::ImportStatement`

* Deconstruct into parts `{ candidate_hash, candidate_receipt, session, statements }`.
* If the session is earlier than `state.highest_session - DISPUTE_WINDOW`, return.
* If there is an entry in the `state.overlay`, load that. Otherwise, load from underlying DB by querying `(session, "candidate-votes", candidate_hash). If that does not exist, create fresh with the given candidate receipt.
* If candidate votes is empty and the statements only contain dispute-specific votes, return.
* Otherwise, if there is already an entry from the validator in the respective `valid` or `invalid` field of the `CandidateVotes`, return.
* Add an entry to the respective `valid` or `invalid` list of the `CandidateVotes` for each statement in `statements`. 
* Write the `CandidateVotes` to the `state.overlay`.
* If the both `valid` and `invalid` lists now have non-zero length where previously one or both had zero length, the candidate is now freshly disputed.
* If freshly disputed, load `"active-disputes"` and add the candidate hash and session index. Also issue a [`DisputeParticipationMessage::Participate`][DisputeParticipationMessage].
* If the dispute now has supermajority votes in the "valid" direction, according to the `SessionInfo` of the dispute candidate's session, remove from `"active-disputes"`.
* If the dispute now has supermajority votes in the "invalid" direction, there is no need to do anything explicitly. The actual rollback will be handled during the active leaves update by observing digests from the runtime.
* Write `"active-disputes"`

### On `DisputeCoordinatorMessage::ActiveDisputes`

* Load `"active-disputes"` and return the data contained within.

### On `DisputeCoordinatorMessage::QueryCandidateVotes`

* Load from the `state.overlay`, and return the data if `Some`. 
* Otherwise, load `"candidate-votes"` and return the data within or `None` if missing.

### On `DisputeCoordinatorMessage::IssueLocalStatement`

* Deconstruct into parts `{ session_index, candidate_hash, candidate_receipt, is_valid }`.
* Construct a [`DisputeStatement`][DisputeStatement] based on `Valid` or `Invalid`, depending on the parameterization of this routine. 
* Sign the statement with each key in the `SessionInfo`'s list of parachain validation keys which is present in the keystore, except those whose indices appear in `voted_indices`. This will typically just be one key, but this does provide some future-proofing for situations where the same node may run on behalf multiple validators. At the time of writing, this is not a use-case we support as other subsystems do not invariably provide this guarantee.

### On `DisputeCoordinatorMessage::DetermineUndisputedChain`

* Load `"active-disputes"`.
* Deconstruct into parts `{ base_number, block_descriptions, rx }`
* Starting from the beginning of `block_descriptions`:
  1. Check the `ActiveDisputes` for a dispute of each candidate in the block description.
  1. If there is a dispute, exit the loop.
* For the highest index `i` reached in the `block_descriptions`, send `(base_number + i + 1, block_hash)` on the channel, unless `i` is 0, in which case `None` should be sent. The `block_hash` is determined by inspecting `block_descriptions[i]`.

### Periodically

* Flush the `state.overlay` to the DB, writing all entries within
* Clear `state.overlay`.

[DisputeTypes]: ../../types/disputes.md
[DisputeStatement]: ../../types/disputes.md#disputestatement
[DisputeCoordinatorMessage]: ../../types/overseer-protocol.md#dispute-coordinator-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
