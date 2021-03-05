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

### On `OverseerSignal::Conclude`

Flush the overlay to DB and conclude.

### On `OverseerSignal::BlockFinalized`

Do nothing.

### On `DisputeCoordinatorMessage::ImportStatement`

* Deconstruct into parts `{ candidate_hash, session, statement, validator_index, validator_signature }`.
* If the session is earlier than `state.highest_session - DISPUTE_WINDOW`, return.
* If there is an entry in the `state.overlay`, load that. Otherwise, load from underlying DB by querying `(session, "candidate-votes", candidate_hash). If that does not exist, create fresh.
* If candidate votes is empty and the statement is a dispute-specific vote, return.
* Otherwise, if there is already an entry from the validator in the respective `valid` or `invalid` field of the `CandidateVotes`, return.
* Add an entry to the respective `valid` or `invalid` list of the `CandidateVotes`. 
* Write the `CandidateVotes` to the `state.overlay`.
* If the added-to (`valid` or `invalid`) list now has length `1` and the other list has non-zero length, this candidate is now disputed. 
* If freshly disputed, load `"disputed"`, add the candidate hash and session index, and write `"disputed"`. Also issue a [`DisputeParticipationMessage::participate`][DisputeParticipationMessage] TODO [now].

### Periodically

* Flush the `state.overlay` to the DB, writing all entries within
* Clear `state.overlay`.

[DisputeTypes]: ../../types/disputes.md
[DisputeCoordinatorMessage]: ../../types/overseer-protocol.md#dispute-coordinator-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
