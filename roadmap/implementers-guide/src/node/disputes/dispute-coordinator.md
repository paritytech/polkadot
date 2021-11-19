# Dispute Coordinator

This is the central subsystem of the node-side components which participate in disputes. This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions. Votes older than this session window are pruned.

This subsystem will be the point which produce dispute votes, either positive or negative, based on locally-observed validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from another node, this will trigger participation in the dispute.

## Database Schema

We use an underlying Key-Value database where we assume we have the following operations available:
  * `write(key, value)`
  * `read(key) -> Option<value>`
  * `iter_with_prefix(prefix) -> Iterator<(key, value)>` - gives all keys and values in lexicographical order where the key starts with `prefix`.

We use this database to encode the following schema:

```rust
("candidate-votes", SessionIndex, CandidateHash) -> Option<CandidateVotes>
"recent-disputes" -> RecentDisputes
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

// The status of the dispute.
enum DisputeStatus {
  // Dispute is still active.
  Active,
  // Dispute concluded positive (2/3 supermajority) along with what
  // timestamp it concluded at.
  ConcludedPositive(Timestamp),
  // Dispute concluded negative (2/3 supermajority, takes precedence over
  // positive in the case of many double-votes).
  ConcludedNegative(Timestamp),
}

struct RecentDisputes {
    // sorted by session index and then by candidate hash.
    disputed: Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
}
```

## Protocol

Input: [`DisputeCoordinatorMessage`][DisputeCoordinatorMessage]

Output:
  - [`RuntimeApiMessage`][RuntimeApiMessage]

## Functionality

This assumes a constant `DISPUTE_WINDOW: SessionWindowSize`. This should correspond to at least 1 day.

Ephemeral in-memory state:

```rust
struct State {
    keystore: KeyStore,
    highest_session: SessionIndex,
}
```

### On startup

Check DB for recorded votes for non concluded disputes we have not yet
recorded a local statement for.
For all of those initiate dispute participation.

### On `OverseerSignal::ActiveLeavesUpdate`

For each leaf in the leaves update:

* Fetch the session index for the child of the block with a [`RuntimeApiMessage::SessionIndexForChild`][RuntimeApiMessage].
* If the session index is higher than `state.highest_session`:
  * update `state.highest_session`
  * remove everything with session index less than `state.highest_session - DISPUTE_WINDOW` from the `"recent-disputes"` in the DB.
  * Use `iter_with_prefix` to remove everything from `"earliest-session"` up to `state.highest_session - DISPUTE_WINDOW` from the DB under `"candidate-votes"`.
  * Update `"earliest-session"` to be equal to `state.highest_session - DISPUTE_WINDOW`.
* For each new block, explicitly or implicitly, under the new leaf, scan for a dispute digest which indicates a rollback. If a rollback is detected, use the `ChainApi` subsystem to blacklist the chain.
* For each new block, use the `RuntimeApi` to obtain a `ScrapedOnChainVotes` and handle them as if they were provided by means of a incoming `DisputeCoordinatorMessage::ImportStatement` message.
  * In the case of a concluded dispute, there are some cases that do not guarantee the presence of a `CandidateReceipt`, where handling has to be defered <https://github.com/paritytech/polkadot/issues/4011>.

### On `OverseerSignal::Conclude`

Exit gracefully.

### On `OverseerSignal::BlockFinalized`

Do nothing.

### On `DisputeCoordinatorMessage::ImportStatement`

1. Deconstruct into parts `{ candidate_hash, candidate_receipt, session, statements }`.
2. If the session is earlier than `state.highest_session - DISPUTE_WINDOW`,
   respond with `ImportStatementsResult::InvalidImport` and return.
3. Load from underlying DB by querying `("candidate-votes", session,
   candidate_hash)`.  If that does not exist, create fresh with the given
   candidate receipt.
4. If candidate votes is empty and the statements only contain dispute-specific
   votes, respond with `ImportStatementsResult::InvalidImport` and return.
5. Otherwise, if there is already an entry from the validator in the respective
  `valid` or `invalid` field of the `CandidateVotes`,  respond with
  `ImportStatementsResult::ValidImport` and return.
6. Add an entry to the respective `valid` or `invalid` list of the
   `CandidateVotes` for each statement in `statements`.
7. If the both `valid` and `invalid` lists now became non-zero length where
   previously one or both had zero length, the candidate is now freshly
   disputed.
8. If the candidate is not freshly disputed as determined by 7, continue with
   10. If it is freshly disputed now, load `"recent-disputes"` and add the
   candidate hash and session index. Then, if we have local statements with
   regards to that candidate,  also continue with 10. Otherwise proceed with 9.
9. Issue a
   [`DisputeParticipationMessage::Participate`][DisputeParticipationMessage].
   Wait for response on the `report_availability` oneshot. If available, continue
   with 10. If not send back `ImportStatementsResult::InvalidImport` and return.
10. Write the `CandidateVotes` to the underyling DB.
11. Send back `ImportStatementsResult::ValidImport`.
12. If the dispute now has supermajority votes in the "valid" direction,
    according to the `SessionInfo` of the dispute candidate's session, the
    `DisputeStatus` should be set to `ConcludedPositive(now)` unless it was
    already `ConcludedNegative`.
13. If the dispute now has supermajority votes in the "invalid" direction,
    the `DisputeStatus` should be set to `ConcludedNegative(now)`. If it
    was `ConcludedPositive` before, the timestamp `now` should be copied
    from the previous status. It will be pruned after some time and all chains
    containing the disputed block will be reverted by the runtime and
    chain-selection subsystem.
14. Write `"recent-disputes"`

### On `DisputeCoordinatorMessage::ActiveDisputes`

* Load `"recent-disputes"` and filter out any disputes which have been concluded for over 5 minutes. Return the filtered data

### On `DisputeCoordinatorMessage::QueryCandidateVotes`

* Load `"candidate-votes"` for every `(SessionIndex, CandidateHash)` in the query and return data within each `CandidateVote`.
  If a particular `candidate-vote` is missing, that particular request is omitted from the response.

### On `DisputeCoordinatorMessage::IssueLocalStatement`

* Deconstruct into parts `{ session_index, candidate_hash, candidate_receipt, is_valid }`.
* Construct a [`DisputeStatement`][DisputeStatement] based on `Valid` or `Invalid`, depending on the parameterization of this routine.
* Sign the statement with each key in the `SessionInfo`'s list of parachain validation keys which is present in the keystore, except those whose indices appear in `voted_indices`. This will typically just be one key, but this does provide some future-proofing for situations where the same node may run on behalf multiple validators. At the time of writing, this is not a use-case we support as other subsystems do not invariably provide this guarantee.
* Write statement to DB.
* Send a `DisputeDistributionMessage::SendDispute` message to get the vote
  distributed to other validators.

### On `DisputeCoordinatorMessage::DetermineUndisputedChain`

* Load `"recent-disputes"`.
* Deconstruct into parts `{ base_number, block_descriptions, rx }`
* Starting from the beginning of `block_descriptions`:
  1. Check the `RecentDisputes` for a dispute of each candidate in the block description.
  1. If there is a dispute which is active or concluded negative, exit the loop.
* For the highest index `i` reached in the `block_descriptions`, send `(base_number + i + 1, block_hash)` on the channel, unless `i` is 0, in which case `None` should be sent. The `block_hash` is determined by inspecting `block_descriptions[i]`.

[DisputeTypes]: ../../types/disputes.md
[DisputeStatement]: ../../types/disputes.md#disputestatement
[DisputeCoordinatorMessage]: ../../types/overseer-protocol.md#dispute-coordinator-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
