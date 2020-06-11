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
  receipt: AbridgedCandidateReceipt,
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
```

> TODO: `CandidateReceipt` and `AbridgedCandidateReceipt` can contain code upgrades which make them very large. the code entries should be split into a different storage map with infrequent access patterns

## Session Change

1. Clear out all candidates pending availability.
1. Clear out all validator bitfields.

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
* `process_candidates(BackedCandidates, scheduled: Vec<CoreAssignment>)`:
  1. check that each candidate corresponds to a scheduled core and that they are ordered in ascending order by `ParaId`.
  1. Ensure that any code upgrade scheduled by the candidate does not happen within `config.validation_upgrade_frequency` of the currently scheduled upgrade, if any, comparing against the value of `Paras::FutureCodeUpgrades` for the given para ID.
  1. check the backing of the candidate using the signatures and the bitfields.
  1. create an entry in the `PendingAvailability` map for each backed candidate with a blank `availability_votes` bitfield.
  1. Return a `Vec<CoreIndex>` of all scheduled cores of the list of passed assignments that a candidate was successfully backed for, sorted ascending by CoreIndex.
* `enact_candidate(relay_parent_number: BlockNumber, AbridgedCandidateReceipt)`:
  1. If the receipt contains a code upgrade, Call `Paras::schedule_code_upgrade(para_id, code, relay_parent_number + config.validationl_upgrade_delay)`.
    > TODO: Note that this is safe as long as we never enact candidates where the relay parent is across a session boundary. In that case, which we should be careful to avoid with contextual execution, the configuration might have changed and the para may de-sync from the host's understanding of it.
  1. Call `Paras::note_new_head` using the `HeadData` from the receipt and `relay_parent_number`.
* `collect_pending`:

  ```rust
    fn collect_pending(f: impl Fn(CoreIndex, BlockNumber) -> bool) -> Vec<u32> {
      // sweep through all paras pending availability. if the predicate returns true, when given the core index and
      // the block number the candidate has been pending availability since, then clean up the corresponding storage for that candidate.
      // return a vector of cleaned-up core IDs.
    }
  ```
