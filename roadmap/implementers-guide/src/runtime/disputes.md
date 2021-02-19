# Disputes Module

After a backed candidate is made available, it is included and proceeds into an acceptance period during which validators are randomly selected to do (secondary) approval checks of the parablock. Any reports disputing the validity of the candidate will cause escalation, where even more validators are requested to check the block, and so on, until either the parablock is determined to be invalid or valid. Those on the wrong side of the dispute are slashed and, if the parablock is deemed invalid, the relay chain is rolled back to a point before that block was included.

However, this isn't the end of the story. We are working in a forkful blockchain environment, which carries three important considerations:

1. For security, validators that misbehave shouldn't only be slashed on one fork, but on all possible forks. Validators that misbehave shouldn't be able to create a new fork of the chain when caught and get away with their misbehavior.
1. It is possible (and likely) that the parablock being contested has not appeared on all forks.
1. If a block author believes that there is a disputed parablock on a specific fork that will resolve to a reversion of the fork, that block author is better incentivized to build on a different fork which does not include that parablock.

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
// Whether the chain is frozen or not. Starts as `false`. When this is `true`,
// the chain will not accept any new parachain blocks for backing or inclusion.
// It can only be set back to `false` by governance intervention.
Frozen: bool,
```

Configuration:

```rust
/// How many sessions before the current that disputes should be accepted for.
DisputePeriod: SessionIndex;
/// How long after conclusion to accept statements.
PostConclusionAcceptancePeriod: BlockNumber;
/// How long is takes for a dispute to conclude by time-out, if no supermajority is reached.
ConclusionByTimeOutPeriod: BlockNumber;
```

## Session Change

1. If the current session is not greater than `dispute_period + 1`, nothing to do here.
1. Set `pruning_target = current_session - dispute_period - 1`. We add the extra `1` because we want to keep things for `dispute_period` _full_ sessions. The stuff at the end of the most recent session has been around for ~0 sessions, not ~1.
1. If `LastPrunedSession` is `None`, then set `LastPrunedSession` to `Some(pruning_target)` and return.
1. Otherwise, clear out all disputes and included candidates in the range `last_pruned..=pruning_target` and set `LastPrunedSession` to `Some(pruning_target)`.

## Block Initialization

1. Iterate through all disputes. If any have not concluded and started more than `ConclusionByTimeOutPeriod` blocks ago, set them to `Concluded` and mildly punish all validators associated, as they have failed to distribute available data.

## Routines

* `provide_multi_dispute_data(MultiDisputeStatementSet) -> Vec<(SessionIndex, Hash)>`:
  1. Fail if any disputes in the set are duplicate or concluded before the `PostConclusionAcceptancePeriod` window relative to now.
  1. Pass on each dispute statement set to `provide_dispute_data`, propagating failure.
  1. Return a list of all candidates who just had disputes initiated.

* `provide_dispute_data(DisputeStatementSet) -> bool`: Provide data to an ongoing dispute or initiate a dispute.
  1. All statements must be issued under the correct session for the correct candidate. 
  1. `SessionInfo` is used to check statement signatures and this function should fail if any signatures are invalid.
  1. If there is no dispute under `Disputes`, create a new `DisputeState` with blank bitfields.
  1. If `concluded_at` is `Some`, and is `concluded_at + PostConclusionAcceptancePeriod < now`, return false.
  1. Import all statements into the dispute. This should fail if any disputes are duplicate; if the corresponding bit for the corresponding validator is set in the dispute already.
  1. If `concluded_at` is `None`, reward all statements slightly less.
  1. If `concluded_at` is `Some`, reward all statements slightly less.
  1. If either side now has supermajority, slash the other side. This may be both sides, and we support this possibility in code, but note that this requires validators to participate on both sides which has negative expected value. Set `concluded_at` to `Some(now)`.
  1. If just concluded against the candidate and the `Included` map contains `(session, candidate)`: invoke `revert_and_freeze` with the stored block number.
  1. Return true if just initiated, false otherwise.

* `disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState)>`: Get a list of all disputes and info about dispute state.
  1. Iterate over all disputes in `Disputes`. Set the flag according to `concluded`.

* `note_included(SessionIndex, CandidateHash, included_in: BlockNumber)`:
  1. Add `(SessionIndex, CandidateHash)` to the `Included` map with `included_in - 1` as the value.
  1. If there is a dispute under `(SessionIndex, CandidateHash)` that has concluded against the candidate, invoke `revert_and_freeze` with the stored block number.

* `could_be_invalid(SessionIndex, CandidateHash) -> bool`: Returns whether a candidate has a live dispute ongoing or a dispute which has already concluded in the negative.

* `is_frozen()`: Load the value of `Frozen` from storage.

* `revert_and_freeze(BlockNumber):
  1. If `is_frozen()` return.
  1. issue a digest in the block header which indicates the chain is to be abandoned back to the stored block number.
  1. Set `Frozen` to true.
