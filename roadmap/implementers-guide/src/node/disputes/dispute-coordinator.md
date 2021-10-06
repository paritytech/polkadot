# Dispute Coordinator

This is the central subsystem of the node-side components which participate in disputes. This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions. Votes older than this session window are pruned.

This subsystem will be the point which produce dispute votes, either positive or negative, based on locally-observed validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from another node, this will trigger the [dispute participation](dispute-participation.md) subsystem to recover and validate the block and call back to this subsystem.

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
  - [`DisputeParticipationMessage`][DisputeParticipationMessage]

## Functionality

This assumes a constant `DISPUTE_WINDOW: SessionIndex`. This should correspond to at least 1 day.

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
For all of those send `DisputeParticipationMessage::Participate` message to
dispute participation subsystem.

### On `OverseerSignal::ActiveLeavesUpdate`

For each leaf in the leaves update:

* Fetch the session index for the child of the block with a [`RuntimeApiMessage::SessionIndexForChild`][RuntimeApiMessage].
* If the session index is higher than `state.highest_session`:
  * update `state.highest_session`
  * remove everything with session index less than `state.highest_session - DISPUTE_WINDOW` from the `"recent-disputes"` in the DB.
  * Use `iter_with_prefix` to remove everything from `"earliest-session"` up to `state.highest_session - DISPUTE_WINDOW` from the DB under `"candidate-votes"`.
  * Update `"earliest-session"` to be equal to `state.highest_session - DISPUTE_WINDOW`.
* For each new block, explicitly or implicitly, under the new leaf, scan for a dispute digest which indicates a rollback. If a rollback is detected, use the `ChainApi` subsystem to blacklist the chain.

### On `OverseerSignal::Conclude`

Exit gracefully.

### On `OverseerSignal::BlockFinalized`

Do nothing.

### On `DisputeCoordinatorMessage::ImportStatements`

#### Ordering/filtering

Before actually acting upon a statements import we do check a few things and
order incoming imports.

First we check whether the incoming votes concern a dispute we already
participated in. If that is the case then there is no work to do from our side
(no availability recovery, no validity checking) and it is safe to import the
candidate right away (see next section).

If the candidate is not within recent disputes, but all queues are empty, we
also go to importing the candidate, as we don't have anything better to do right
now.

If queues are non empty, we check whether we have any record of the candidate
gotten included on some chain we have seen in the past. If so, we know the
statements are not pure spam and put the votes on the priority queue ordered by
age (explained below).

If we have not seen the candidate gotten included somewhere, then we either have
been offline/or missed the fork for some reason - in any case, such a candidate
might be spam so we put it on the low priority queue which gets processed on a
best effort basis.

If we have seen the candidate included, but the candidate has a relay parent
that has already been finalized, also put the candidate on the best effort
queue. (We can't prevent any potential harm anymore, so we should prioritize
disputes concerning non finalized blocks.)

###### High priority queue

For statements concerning a candidate that we have seen included, we put those
statements on a high priority queue, which is conceptually organized like this:

```rust

BTreeMap<CandidateComparator, (Candidate, HashSet<(SignedDisputeStatement,
      ValidatorIndex)>)>

struct CandidateComparator {
  block_number: BlockNumber,
  para_id: ParaId,
  relay_parent: Hash
}

impl Ord for CandidateComparator {
  fn cmp(&self, other: &Self) -> Ordering {
      match self.block_number.cmp(other.block_number) {
        Ordering::Equal => (),
        o => return o,
      }
      match self.block_number.cmp(other.para_id) {
        Ordering::Equal => (),
        o => return o,
      }
      self.relay_parent.cmp(other.relay_parent)
    }
}
```
TODO: Better use block number of block including the candidate to account for
contextual execution.
TODO: https://github.com/paritytech/srlabs_findings/issues/136
So we order first by block number of the relay parent of the candidate, and then
by `ParaId` and only afterwards by its relay parent hash. This way we get a
deterministic ordering that should be the same on all validators. The size of
that map will be limited to some reasonable value. The number of votes for each
candidate can easily be limited to twice the number validators, so everyone can
equivocate, if they desire to do so.

Reasoning for the ordering: By checking the block number first, we make sure we
are participating in the oldest disputes first, which is important as they are
the most time critical. By comparing the para id second, we make sure we will
treat multiple forks fairly. E.g. we will participate for candidate of parachain
A on fork 1, then on parachain A on fork 2 and so on. This is desirable, so it
cannot happen that for example an abandoned chain's disputes starves disputes on
a better chain, which is actually relevant. Finally, the ordering by relay
parent hash, ensures we do have deterministic ordering in the event of forks.

The most important part is, that we get deterministic ordering across validators
in order for them to pull in the same direction and get disputes to conclude,
even in the event of lots of concurrent disputes getting initiated due to some
malicious activity.

Note that even with this mechanism a validator could try to raise lots of
disputes which will all resolve in favour of the corresponding candidate, in
order to delay the checking of an actual malicious candidate, provoking this
dispute to timeout. We can do two things to mitigate this:

1. Revert the chain in case a dispute times out.
2. Have validators get slashed significantly enough on falsely disputed
   candidates and make sure we stop accepting disputes from them
   eventually (once their stake became too low).

##### Low Priority Queue

To be defined: Depending on how likely it is that we don't see a valid candidate
included but are still able to look up its block number, it might either make
sense to try to maintain an equivalent ordering as in the high priority queue or
just order by candidate hash for example.


##### Reasoning / Considerations of this setup

If multiple disputes are in flight and get queued, we might
- Responses to onshot might be late
- Explain network effects
- Slashing for false negative
- Forks: Disputes might technically be older, but on an abandoned fork -
  shouldn't we de-prioritize those?
- We cannot fully protect from spam this way, we also rely on the actual import
  to ignore votes from validators which are disabled/have been slashed too much
  already. Otherwise, if a single validator is constantly disputing all cores,
  we are still in trouble. The ordering is still essential so we finish disputes
  and validators actually get slashed.

  Unrelated:
    In case of block producers - we don't participate, do we still try to
    recover availability - NOOOOOOO! Harmfull?! - Could fill up disk.

#### Processing
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
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
