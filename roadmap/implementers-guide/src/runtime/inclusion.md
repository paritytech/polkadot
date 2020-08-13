# Inclusion Module

The inclusion module is responsible for inclusion and availability of scheduled parachains and parathreads.

## Storage

Helper structs:

```rust
struct AvailabilityBitfield {
  bitfield: BitVec, // one bit per core.
  submitted_at: BlockNumber, // for accounting, as meaning of bits may change over time.
}

struct CandidatePendingAvailability {
  core: CoreIndex, // availability core
  descriptor: CandidateDescriptor,
  availability_votes: Bitfield, // one bit per validator.
  relay_parent_number: BlockNumber, // number of the relay-parent.
  backed_in_number: BlockNumber,
}
```

Storage Layout:

```rust
/// The latest bitfield for each validator, referred to by index.
bitfields: map ValidatorIndex => AvailabilityBitfield;
/// Candidates pending availability.
PendingAvailability: map ParaId => CandidatePendingAvailability;
/// The commitments of candidates pending availability, by ParaId.
PendingAvailabilityCommitments: map ParaId => CandidateCommitments;

/// The current validators, by their parachain session keys.
Validators: Vec<ValidatorId>;

/// The current session index.
CurrentSessionIndex: SessionIndex;
```

## Session Change

1. Clear out all candidates pending availability.
1. Clear out all validator bitfields.
1. Update `Validators` with the validators from the session change notification.
1. Update `CurrentSessionIndex` with the session index from the session change notification.

## Routines

All failed checks should lead to an unrecoverable error making the block invalid.

* `process_bitfields(Bitfields, core_lookup: Fn(CoreIndex) -> Option<ParaId>)`:
  1. check that the number of bitfields and bits in each bitfield is correct.
  1. check that there are no duplicates
  1. check all validator signatures.
  1. apply each bit of bitfield to the corresponding pending candidate. looking up parathread cores using the `core_lookup`. Disregard bitfields that have a `1` bit for any free cores.
  1. For each applied bit of each availability-bitfield, set the bit for the validator in the `CandidatePendingAvailability`'s `availability_votes` bitfield. Track all candidates that now have >2/3 of bits set in their `availability_votes`. These candidates are now available and can be enacted.
  1. For all now-available candidates, invoke the `enact_candidate` routine with the candidate and relay-parent number.
  1. > TODO: pass it onwards to `Validity` module.
  1. Return a list of freed cores consisting of the cores where candidates have become available.
* `process_candidates(BackedCandidates, scheduled: Vec<CoreAssignment>, group_validators: Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>)`:
  1. check that each candidate corresponds to a scheduled core and that they are ordered in the same order the cores appear in assignments in `scheduled`.
  1. check that `scheduled` is sorted ascending by `CoreIndex`, without duplicates.
  1. check that there is no candidate pending availability for any scheduled `ParaId`.
  1. check that each candidate's `validation_data_hash` corresponds to a `PersistedValidationData` computed from the current state.
    > NOTE: With contextual execution in place, validation data will be obtained as of the state of the context block. However, only the state of the current block can be used for such a query.
  1. If the core assignment includes a specific collator, ensure the backed candidate is issued by that collator.
  1. Ensure that any code upgrade scheduled by the candidate does not happen within `config.validation_upgrade_frequency` of `Paras::last_code_upgrade(para_id, true)`, if any, comparing against the value of `Paras::FutureCodeUpgrades` for the given para ID.
  1. Check the collator's signature on the candidate data.
  1. check the backing of the candidate using the signatures and the bitfields, comparing against the validators assigned to the groups, fetched with the `group_validators` lookup.
  1. check that the upward messages, when combined with the existing queue size, are not exceeding `config.max_upward_queue_count` and `config.watermark_upward_queue_size` parameters.
  1. call `Router::check_processed_downward_messages(para, commitments.processed_downward_messages)` to check that the DMQ is properly drained.
  1. call `Router::check_hrmp_watermark(para, commitments.hrmp_watermark)` for each candidate to check rules of processing the HRMP watermark.
  1. check that in the commitments of each candidate the horizontal messages are sorted by ascending recipient ParaId and there is no two horizontal messages have the same recipient.
  1. using `Router::verify_outbound_hrmp(sender, commitments.horizontal_messages)` ensure that the each candidate send a valid set of horizontal messages
  1. create an entry in the `PendingAvailability` map for each backed candidate with a blank `availability_votes` bitfield.
  1. create a corresponding entry in the `PendingAvailabilityCommitments` with the commitments.
  1. Return a `Vec<CoreIndex>` of all scheduled cores of the list of passed assignments that a candidate was successfully backed for, sorted ascending by CoreIndex.
* `enact_candidate(relay_parent_number: BlockNumber, CommittedCandidateReceipt)`:
  1. If the receipt contains a code upgrade, Call `Paras::schedule_code_upgrade(para_id, code, relay_parent_number + config.validationl_upgrade_delay)`.
    > TODO: Note that this is safe as long as we never enact candidates where the relay parent is across a session boundary. In that case, which we should be careful to avoid with contextual execution, the configuration might have changed and the para may de-sync from the host's understanding of it.
  1. call `Router::queue_upward_messages` for each backed candidate, using the [`UpwardMessage`s](../types/messages.md#upward-message) from the [`CandidateCommitments`](../types/candidate.md#candidate-commitments).
  1. call `Router::queue_outbound_hrmp` with the para id of the candidate and the list of horizontal messages taken from the commitment,
  1. call `Router::prune_hrmp` with the para id of the candiate and the candidate's `hrmp_watermark`.
  1. call `Router::prune_dmq` with the para id of the candidate and the candidate's `processed_downward_messages`.
  1. Call `Paras::note_new_head` using the `HeadData` from the receipt and `relay_parent_number`.
* `collect_pending`:

  ```rust
    fn collect_pending(f: impl Fn(CoreIndex, BlockNumber) -> bool) -> Vec<u32> {
      // sweep through all paras pending availability. if the predicate returns true, when given the core index and
      // the block number the candidate has been pending availability since, then clean up the corresponding storage for that candidate and the commitments.
      // return a vector of cleaned-up core IDs.
    }
  ```
* `force_enact(ParaId)`: Forcibly enact the candidate with the given ID as though it had been deemed available by bitfields. Is a no-op if there is no candidate pending availability for this para-id. This should generally not be used but it is useful during execution of Runtime APIs, where the changes to the state are expected to be discarded directly after.
* `candidate_pending_availability(ParaId) -> Option<CommittedCandidateReceipt>`: returns the `CommittedCandidateReceipt` pending availability for the para provided, if any.
* `pending_availability(ParaId) -> Option<CandidatePendingAvailability>`: returns the metadata around the candidate pending availability for the para, if any.
