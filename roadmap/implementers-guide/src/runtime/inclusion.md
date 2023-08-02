# Inclusion Pallet

The inclusion module is responsible for inclusion and availability of scheduled parachains. It also manages the UMP dispatch queue of each parachain.

## Storage

Helper structs:

```rust
struct AvailabilityBitfield {
  bitfield: BitVec, // one bit per core.
  submitted_at: BlockNumber, // for accounting, as meaning of bits may change over time.
}

struct CandidatePendingAvailability {
  core: CoreIndex, // availability core
  hash: CandidateHash,
  descriptor: CandidateDescriptor,
  availability_votes: Bitfield, // one bit per validator.
  relay_parent_number: BlockNumber, // number of the relay-parent.
  backers: Bitfield, // one bit per validator, set for those who backed the candidate.
  backed_in_number: BlockNumber,
  backing_group: GroupIndex,
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
```

## Config Dependencies

* `MessageQueue`:
  The message queue provides general queueing and processing functionality. Currently it
  replaces the old `UMP` dispatch queue. Other use-cases can be implemented as well by
  adding new variants to `AggregateMessageOrigin`. Normally it should be set to an instance
  of the `MessageQueue` pallet.

## Session Change

1. Clear out all candidates pending availability.
1. Clear out all validator bitfields.

Optional:
1. The UMP queue of all outgoing paras can be "swept". This would prevent the dispatch queue from automatically being serviced. It is a consideration for the chain and specific behaviour is not defined.

## Initialization

No initialization routine runs for this module. However, the initialization of the `MessageQueue` pallet will attempt to process any pending UMP messages.


## Routines

All failed checks should lead to an unrecoverable error making the block invalid.

* `process_bitfields(expected_bits, Bitfields, core_lookup: Fn(CoreIndex) -> Option<ParaId>)`:
  1. Call `sanitize_bitfields<true>` and use the sanitized `signed_bitfields` from now on.
  1. Call `sanitize_backed_candidates<true>` and use the sanitized `backed_candidates` from now on.
  1. Apply each bit of bitfield to the corresponding pending candidate, looking up on-demand parachain cores using the `core_lookup`. Disregard bitfields that have a `1` bit for any free cores.
  1. For each applied bit of each availability-bitfield, set the bit for the validator in the `CandidatePendingAvailability`'s `availability_votes` bitfield. Track all candidates that now have >2/3 of bits set in their `availability_votes`. These candidates are now available and can be enacted.
  1. For all now-available candidates, invoke the `enact_candidate` routine with the candidate and relay-parent number.
  1. Return a list of `(CoreIndex, CandidateHash)` from freed cores consisting of the cores where candidates have become available.
* `sanitize_bitfields<T: crate::inclusion::Config>(
    unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
    disputed_bitfield: DisputedBitfield,
    expected_bits: usize,
    parent_hash: T::Hash,
    session_index: SessionIndex,
    validators: &[ValidatorId],
    full_check: FullCheck,
  )`:
  1. check that `disputed_bitfield` has the same number of bits as the `expected_bits`, iff not return early with an empty vec.
  1. each of the below checks is for each bitfield. If a check does not pass the bitfield will be skipped.
  1. check that there are no bits set that reference a disputed candidate.
  1. check that the number of bits is equal to `expected_bits`.
  1. check that the validator index is strictly increasing (and thus also unique).
  1. check that the validator bit index is not out of bounds.
  1. check the validators signature, iff `full_check=FullCheck::Yes`.

* `sanitize_backed_candidates<T: crate::inclusion::Config, F: FnMut(usize, &BackedCandidate<T::Hash>) -> bool>(
    mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
    candidate_has_concluded_invalid_dispute: F,
    scheduled: &[CoreAssignment],
  ) `
  1. filter out any backed candidates that have concluded invalid.
  1. filters backed candidates whom's paraid was scheduled by means of the provided `scheduled` parameter.
  1. sorts remaining candidates with respect to the core index assigned to them.

* `process_candidates(allowed_relay_parents, BackedCandidates, scheduled: Vec<CoreAssignment>, group_validators: Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>)`:
    > For details on `AllowedRelayParentsTracker` see documentation for [Shared](./shared.md) module.
  1. check that each candidate corresponds to a scheduled core and that they are ordered in the same order the cores appear in assignments in `scheduled`.
  1. check that `scheduled` is sorted ascending by `CoreIndex`, without duplicates.
  1. check that the relay-parent from each candidate receipt is one of the allowed relay-parents.
  1. check that there is no candidate pending availability for any scheduled `ParaId`.
  1. check that each candidate's `validation_data_hash` corresponds to a `PersistedValidationData` computed from the state of the context block.
  1. If the core assignment includes a specific collator, ensure the backed candidate is issued by that collator.
  1. Ensure that any code upgrade scheduled by the candidate does not happen within `config.validation_upgrade_cooldown` of `Paras::last_code_upgrade(para_id, true)`, if any, comparing against the value of `Paras::FutureCodeUpgrades` for the given para ID.
  1. Check the collator's signature on the candidate data.
  1. check the backing of the candidate using the signatures and the bitfields, comparing against the validators assigned to the groups, fetched with the `group_validators` lookup, while group indices are computed by `Scheduler` according to group rotation info. 
  1. call `check_upward_messages(config, para, commitments.upward_messages)` to check that the upward messages are valid.
  1. call `Dmp::check_processed_downward_messages(para, commitments.processed_downward_messages)` to check that the DMQ is properly drained.
  1. call `Hrmp::check_hrmp_watermark(para, commitments.hrmp_watermark)` for each candidate to check rules of processing the HRMP watermark.
  1. using `Hrmp::check_outbound_hrmp(sender, commitments.horizontal_messages)` ensure that the each candidate sent a valid set of horizontal messages
  1. create an entry in the `PendingAvailability` map for each backed candidate with a blank `availability_votes` bitfield.
  1. create a corresponding entry in the `PendingAvailabilityCommitments` with the commitments.
  1. Return a `Vec<CoreIndex>` of all scheduled cores of the list of passed assignments that a candidate was successfully backed for, sorted ascending by CoreIndex.
* `enact_candidate(relay_parent_number: BlockNumber, CommittedCandidateReceipt)`:
  1. If the receipt contains a code upgrade, Call `Paras::schedule_code_upgrade(para_id, code, relay_parent_number, config)`.
    > TODO: Note that this is safe as long as we never enact candidates where the relay parent is across a session boundary. In that case, which we should be careful to avoid with contextual execution, the configuration might have changed and the para may de-sync from the host's understanding of it.
  1. Reward all backing validators of each candidate, contained within the `backers` field.
  1. call `receive_upward_messages` for each backed candidate, using the [`UpwardMessage`s](../types/messages.md#upward-message) from the [`CandidateCommitments`](../types/candidate.md#candidate-commitments).
  1. call `Dmp::prune_dmq` with the para id of the candidate and the candidate's `processed_downward_messages`.
  1. call `Hrmp::prune_hrmp` with the para id of the candiate and the candidate's `hrmp_watermark`.
  1. call `Hrmp::queue_outbound_hrmp` with the para id of the candidate and the list of horizontal messages taken from the commitment,
  1. Call `Paras::note_new_head` using the `HeadData` from the receipt and `relay_parent_number`.

* `collect_pending`:

  ```rust
    fn collect_pending(f: impl Fn(CoreIndex, BlockNumber) -> bool) -> Vec<CoreIndex> {
      // sweep through all paras pending availability. if the predicate returns true, when given the core index and
      // the block number the candidate has been pending availability since, then clean up the corresponding storage for that candidate and the commitments.
      // return a vector of cleaned-up core IDs.
    }
  ```
* `force_enact(ParaId)`: Forcibly enact the candidate with the given ID as though it had been deemed available by bitfields. Is a no-op if there is no candidate pending availability for this para-id. This should generally not be used but it is useful during execution of Runtime APIs, where the changes to the state are expected to be discarded directly after.
* `candidate_pending_availability(ParaId) -> Option<CommittedCandidateReceipt>`: returns the `CommittedCandidateReceipt` pending availability for the para provided, if any.
* `pending_availability(ParaId) -> Option<CandidatePendingAvailability>`: returns the metadata around the candidate pending availability for the para, if any.
* `collect_disputed(disputed: Vec<CandidateHash>) -> Vec<CoreIndex>`: Sweeps through all paras pending availability. If the candidate hash is one of the disputed candidates, then clean up the corresponding storage for that candidate and the commitments. Return a vector of cleaned-up core IDs.

These functions were formerly part of the UMP pallet:

* `check_upward_messages(P: ParaId, Vec<UpwardMessage>)`:
    1. Checks that the parachain is not currently offboarding and error otherwise. 
    1. Checks that there are at most `config.max_upward_message_num_per_candidate` messages to be enqueued.
    1. Checks that no message exceeds `config.max_upward_message_size`.
    1. Checks that the total resulting queue size would not exceed `co`.
    1. Verify that queuing up the messages could not result in exceeding the queue's footprint
    according to the config items `config.max_upward_queue_count` and `config.max_upward_queue_size`. The queue's current footprint is provided in `well_known_keys`
    in order to facilitate oraclisation on to the para.

Candidate Enactment:

* `receive_upward_messages(P: ParaId, Vec<UpwardMessage>)`:
    1. Process each upward message `M` in order:
        1. Place in the dispatch queue according to its para ID (or handle it immediately).
