# Disputes Module

After a backed candidate is made available, it is included and proceeds into an acceptance period during which validators are randomly selected to do (secondary) approval checks of the parablock. Any reports disputing the validity of the candidate will cause escalation, where even more validators are requested to check the block, and so on, until either the parablock is determined to be invalid or valid. Those on the wrong side of the dispute are slashed and, if the parablock is deemed invalid, the relay chain is rolled back to a point before that block was included.

However, this isn't the end of the story. We are working in a forkful blockchain environment, which carries three important considerations:

1. For security, validators that misbehave shouldn't only be slashed on one fork, but on all possible forks. Validators that misbehave shouldn't be able to create a new fork of the chain when caught and get away with their misbehavior.
1. It is possible (and likely) that the parablock being contested has not appeared on all forks.
1. If a block author believes that there is a disputed parablock on a specific fork that will resolve to a reversion of the fork, that block author has more incentive to build on a different fork which does not include that parablock.

This means that in all likelihood, there is the possibility of disputes that are started on one fork of the relay chain, and as soon as the dispute resolution process starts to indicate that the parablock is indeed invalid, that fork of the relay chain will be abandoned and the dispute will never be fully resolved on that chain.

Even if this doesn't happen, there is the possibility that there are two disputes underway, and one resolves leading to a reversion of the chain before the other has concluded. In this case we want to both transplant the concluded dispute onto other forks of the chain as well as the unconcluded dispute.

We account for these requirements by having the disputes module handle two kinds of disputes.

1. Local disputes: those contesting the validity of the current fork by disputing a parablock included within it.
1. Remote disputes: a dispute that has partially or fully resolved on another fork which is transplanted to the local fork for completion and eventual slashing.

When a local dispute concludes negatively, the chain needs to be abandoned and reverted back to a block where the state does not contain the bad parablock. We expect that due to the [Approval Checking Protocol](../protocol-approval.md), the current executing block should not be finalized. So we do two things when a local dispute concludes negatively:
1. Freeze the state of parachains so nothing further is backed or included.
1. Issue a digest in the header of the block that signals to nodes that this branch of the chain is to be abandoned.

If, as is expected, the chain is unfinalized, the freeze will have no effect as no honest validator will attempt to build on the frozen chain. However, if the approval checking protocol has failed and the bad parablock is finalized, the freeze serves to put the chain into a governance-only mode.

The storage of this module is designed around tracking [`DisputeState`s](../types/disputes.md#disputestate), updating them with votes, and tracking blocks included by this branch of the relay chain. It also contains a `Frozen` parameter designed to freeze the state of all parachains.

## Storage

Storage Layout:

```rust
LastPrunedSession: Option<SessionIndex>,

// All ongoing or concluded disputes for the last several sessions.
Disputes: double_map (SessionIndex, CandidateHash) -> Option<DisputeState>,
// All included blocks on the chain, as well as the block number in this chain that
// should be reverted back to if the candidate is disputed and determined to be invalid.
Included: double_map (SessionIndex, CandidateHash) -> Option<BlockNumber>,
// Whether the chain is frozen or not. Starts as `None`. When this is `Some`,
// the chain will not accept any new parachain blocks for backing or inclusion,
// and its value indicates the last valid block number in the chain.
// It can only be set back to `None` by governance intervention.
Frozen: Option<BlockNumber>,
```

> `byzantine_threshold` refers to the maximum number `f` of validators which may be byzantine. The total number of validators is `n = 3f + e` where `e in { 1, 2, 3 }`.

## Session Change

1. If the current session is not greater than `config.dispute_period + 1`, nothing to do here.
1. Set `pruning_target = current_session - config.dispute_period - 1`. We add the extra `1` because we want to keep things for `config.dispute_period` _full_ sessions.
   The stuff at the end of the most recent session has been around for a little over 0 sessions, not a little over 1.
1. If `LastPrunedSession` is `None`, then set `LastPrunedSession` to `Some(pruning_target)` and return.
2. Otherwise, clear out all disputes and included candidates entries in the range `last_pruned..=pruning_target` and set `LastPrunedSession` to `Some(pruning_target)`.

## Block Initialization

This is currently a `no op`.

## Routines

* `filter_multi_dispute_data(MultiDisputeStatementSet) -> MultiDisputeStatementSet`:
  1. Takes a `MultiDisputeStatementSet` and filters it down to a `MultiDisputeStatementSet`
    that satisfies all the criteria of `provide_multi_dispute_data`. That is, eliminating
    ancient votes, duplicates and unconfirmed disputes.
    This can be used by block authors to create the final submission in a block which is
    guaranteed to pass the `provide_multi_dispute_data` checks.

* `provide_multi_dispute_data(MultiDisputeStatementSet) -> Vec<(SessionIndex, Hash)>`:
  1. Pass on each dispute statement set to `provide_dispute_data`, propagating failure.
  2. Return a list of all candidates who just had disputes initiated.

* `provide_dispute_data(DisputeStatementSet) -> bool`: Provide data to an ongoing dispute or initiate a dispute.
  1. All statements must be issued under the correct session for the correct candidate.
  1. `SessionInfo` is used to check statement signatures and this function should fail if any signatures are invalid.
  1. If there is no dispute under `Disputes`, create a new `DisputeState` with blank bitfields.
  1. If `concluded_at` is `Some`, and is `concluded_at + config.post_conclusion_acceptance_period < now`, return false.
  2. Import all statements into the dispute. This should fail if any statements are duplicate or if the corresponding bit for the corresponding validator is set in the dispute already.
  3. If `concluded_at` is `None`, reward all statements.
  4. If `concluded_at` is `Some`, reward all statements slightly less.
  5. If either side now has supermajority and did not previously, slash the other side. This may be both sides, and we support this possibility in code, but note that this requires validators to participate on both sides which has negative expected value. Set `concluded_at` to `Some(now)` if it was `None`.
  6. If just concluded against the candidate and the `Included` map contains `(session, candidate)`: invoke `revert_and_freeze` with the stored block number.
  7. Return true if just initiated, false otherwise.

* `disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState)>`: Get a list of all disputes and info about dispute state.
  1. Iterate over all disputes in `Disputes` and collect into a vector.

* `note_included(SessionIndex, CandidateHash, included_in: BlockNumber)`:
  1. Add `(SessionIndex, CandidateHash)` to the `Included` map with `included_in - 1` as the value.
  1. If there is a dispute under `(SessionIndex, CandidateHash)` that has concluded against the candidate, invoke `revert_and_freeze` with the stored block number.

* `concluded_invalid(SessionIndex, CandidateHash) -> bool`: Returns whether a candidate has already concluded a dispute in the negative.

* `is_frozen()`: Load the value of `Frozen` from storage. Return true if `Some` and false if `None`.

* `last_valid_block()`: Load the value of `Frozen` from storage and return. None indicates that all blocks in the chain are potentially valid.

* `revert_and_freeze(BlockNumber)`:
  1. If `is_frozen()` return.
  1. Set `Frozen` to `Some(BlockNumber)` to indicate a rollback to the block number.
  1. Issue a `Revert(BlockNumber + 1)` log to indicate a rollback of the block's child in the header chain, which is the same as a rollback to the block number.

# Disputes filtering

All disputes delivered to the runtime by the client are filtered before the actual import. In this context actual import
means persisted in the runtime storage. The filtering has got two purposes:
- Limit the amount of data saved onchain.
- Prevent persisting malicious dispute data onchain.

*Implementation note*: Filtering is performed in function `filter_dispute_data` from `Disputes` pallet.

The filtering is performed on the whole statement set which is about to be imported onchain. The following filters are
applied:
1. Remove ancient disputes - if a dispute is concluded before the block number indicated in `OLDEST_ACCEPTED` parameter
   it is removed from the set. `OLDEST_ACCEPTED` is a runtime configuration option.
   *Implementation note*: `dispute_post_conclusion_acceptance_period` from
   `HostConfiguration` is used in the current Polkadot/Kusama implementation.
2. Remove votes from unknown validators. If there is a vote from a validator which wasn't an authority in the session
   where the dispute was raised - they are removed. Please note that this step removes only single votes instead of
   removing the whole dispute.
3. Remove one sided disputes - if a dispute doesn't contain two opposing votes it is not imported onchain. This serves
   as a measure not to import one sided disputes. A dispute is raised only if there are two opposing votes so if the
   client is not sending them the dispute is a potential spam.
4. Remove unconfirmed disputes - if a dispute contains less votes than the byzantine threshold it is removed. This is
   also a spam precaution. A legitimate client will send only confirmed disputes to the runtime.

# Rewards and slashing

After the disputes are filtered the validators participating in the disputes are rewarded and more importantly the
offenders are slashed. Generally there can be two types of punishments:
* "against valid" - the offender claimed that a valid candidate is invalid.
* "for invalid" - the offender claimed that an invalid candidate is valid.

A dispute might be inconclusive. This means that it has timed out without being confirmed. A confirmed dispute is one
containing votes more than the byzantine threshold (1/3 of the active validators). Validators participating in
inconclusive disputes are not slashed. Thanks to the applied filtering (described in the previous section) one can be
confident that there are no spam disputes in the runtime. So if a validator is not voting it is due to another reason
(e.g. being under DoS attack). There is no reason to punish such validators with a slash.

*Implementation note*: Slashing is performed in `process_checked_dispute_data` from `Disputes` pallet.