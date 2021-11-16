# `ParaInherent`

This module is responsible for providing all data given to the runtime by the block author to the various parachains modules. The entry-point is mandatory, in that it must be invoked exactly once within every block, and it is also "inherent", in that it is provided with no origin by the block author. The data within it carries its own authentication; i.e. the data takes the form of signed statements by validators. If any of the steps within fails, the entry-point is considered as having failed and the block will be invalid.

This module does not have the same initialization/finalization concerns as the others, as it only requires that entry points be triggered after all modules have initialized and that finalization happens after entry points are triggered. Both of these are assumptions we have already made about the runtime's order of operations, so this module doesn't need to be initialized or finalized by the `Initializer`.

There are a couple of important notes to the operations in this inherent as they relate to disputes.

1. We don't accept bitfields or backed candidates if in "governance-only" mode from having a local dispute conclude on this fork.
1. When disputes are initiated, we remove the block from pending availability. This allows us to roll back chains to the block before blocks are included as opposed to backing. It's important to do this before processing bitfields.
1. `Inclusion::collect_disputed` is kind of expensive so it's important to gate this on whether there are actually any new disputes. Which should be never.
1. And we don't accept parablocks that have open disputes or disputes that have concluded against the candidate. It's important to import dispute statements before backing, but this is already the case as disputes are imported before processing bitfields.

## Storage

```rust
/// Whether the para inherent was included or not.
Included: Option<()>,
```

```rust
/// Scraped on chain votes to be used in disputes off-chain.
OnChainVotes: Option<ScrapedOnChainVotes>,
```

## Finalization

1. Take (get and clear) the value of `Included`. If it is not `Some`, throw an unrecoverable error.

## Entry Points

* `enter`: This entry-point accepts one parameter: [`ParaInherentData`](../types/runtime.md#ParaInherentData).
    1. Ensure the origin is none.
    1. Ensure `Included` is set as `None`.
    1. Set `Included` as `Some`.
    1. Unpack `ParachainsInherentData` into `signed_bitfields`, `backed_candidates`, `parent_header`, and `disputes`.
    1. Hash the parent header and make sure that it corresponds to the block hash of the parent (tracked by the `frame_system` FRAME module).
    1. Calculate the `candidate_weight`, `bitfields_weight`, and `disputes_weight`.
    1. If the sum of `candidate_weight`, `bitfields_weight`, and `disputes_weight` is greater than the max block weight we do the following with the goal of prioritizing the inclusion of disputes without making it game-able by block authors:
      1. clear `bitfields` and set `bitfields_weight` equal to 0.
      1. clear `backed_candidates` and set `candidate_weight` equal to 0.
      1. invoke `limit_disputes` on the `disputes` with the max block weight iff the disputes weight is greater than the max block weight.
    1. Invoke `Disputes::provide_multi_dispute_data`.
    1. If `Disputes::is_frozen`, return.
    1. If there are any concluded disputes from the current session, invoke `Inclusion::collect_disputed` with the disputed candidates. Annotate each returned core with `FreedReason::Concluded`, sort them, and invoke `Scheduler::free_cores` with them.
    1. The `Bitfields` are first forwarded to the `Inclusion::process_bitfields` routine, returning a set included candidates and the respective freed cores. Provide the number of availability cores (`Scheduler::availability_cores().len()`) as the expected number of bits and a `Scheduler::core_para` as a core-lookup to the `process_bitfields` routine. Annotate each of these freed cores with `FreedReason::Concluded`.
    1. For each freed candidate from the `Inclusion::process_bitfields` call, invoke `Disputes::note_included(current_session, candidate)`.
    1. If `Scheduler::availability_timeout_predicate` is `Some`, invoke `Inclusion::collect_pending` using it and annotate each of those freed cores with `FreedReason::TimedOut`.
    1. Combine and sort the the bitfield-freed cores and the timed-out cores.
    1. Invoke `Scheduler::clear`
    1. Invoke `Scheduler::schedule(freed_cores, System::current_block())`
    1. Extract `parent_storage_root` from the parent header,
    1. If `Disputes::concluded_invalid(current_session, candidate)` is true for any of the `backed_candidates`, fail.
    1. Invoke the `Inclusion::process_candidates` routine with the parameters `(parent_storage_root, backed_candidates, Scheduler::scheduled(), Scheduler::group_validators)`.
    1. Deconstruct the returned `ProcessedCandidates` value into `occupied` core indices, and backing validators by candidate `backing_validators_per_candidate` represented by `Vec<(CandidateReceipt, Vec<(ValidatorIndex, ValidityAttestation)>)>`.
    1. Set `OnChainVotes` to `ScrapedOnChainVotes`, based on the `current_session`, concluded `disputes`, and `backing_validators_per_candidate`.
    1. Call `Scheduler::occupied` using the `occupied` core indices of the returned  above, first sorting the list of assigned core indices.
    1. Call the `Ump::process_pending_upward_messages` routine to execute all messages in upward dispatch queues.
    1. If all of the above succeeds, set `Included` to `Some(())`.


* `create_inherent`: This entry-point accepts one parameter: `InherentData`.
  1. Invoke [`create_inherent_inner(InherentData)`](#routines), the unit testable logic for filtering and sanitzing the inherent data used when invoking `enter`. Save the result as `inherent_data`.
  1. If the `inherent_data` is an `Err` variant, return the `enter` call signature with all inherent data cleared else return the `enter` call signature with `inherent_data` passed in as the `data` param.

# Routines

* `create_inherent_inner(data: &InherentData) -> Option<ParachainsInherentData<T::Header>>`
  1. Unpack `InherentData` into its parts, `bitfields`, `backed_candidates`, `disputes` and the `parent_header`. If data cannot be unpacked return `None`.
  1. Hash the `parent_header` and make sure that it corresponds to the block hash of the parent (tracked by the `frame_system` FRAME module).
  1. Invoke `Disputes::filter_multi_dispute_data` to remove duplicates et al from `disputes`.
  1. Run the following within a  `with_transaction` closure to avoid side effects (we are essentially replicating the logic that would otherwise happen within `enter` so we can get the filtered bitfields and the `concluded_invalid_disputes` + `scheduled` to use in filtering the `backed_candidates`.):
    1. Invoke `Disputes::provide_multi_dispute_data`.
    1. Collect `current_concluded_invalid_disputes`, the disputed candidate hashes from the current session that have concluded invalid.
    1. Collect `concluded_invalid_disputes`, the disputed candidate hashes from the given `backed_candidates`.
    1. Invoke `Inclusion::collect_disputed` with the newly disputed candidates. Annotate each returned core with `FreedReason::Concluded`, sort them, and invoke `Scheduler::free_cores` with them.
    1. Collect filtered `bitfields` by invoking [`sanitize_bitfields<false>`](inclusion.md#Routines).
    1. Collect `freed_concluded` by invoking `update_pending_availability_and_get_freed_cores` on the filtered bitfields.
    1. Collect all `freed` cores by invoking `collect_all_freed_cores` on `freed_concluding`.
    1. Invoke `scheduler::Pallet<T>>::clear()`.
    1. Invoke `scheduler::Pallet<T>>::schedule` with `freed` and the current block number to create the same schedule of the cores that `enter` will create.
    1. Read the new `<scheduler::Pallet<T>>::scheduled()` into `schedule`.
    1. From the `with_transaction` closure return `concluded_invalid_disputes`, `bitfields`, and `scheduled`.
  1. Invoke `sanitize_backed_candidates` using the `scheduled` return from the `with_transaction` and pass the closure `|candidate_hash: CandidateHash| -> bool { DisputesHandler::concluded_invalid(current_session, candidate_hash) }` for the param `candidate_has_concluded_invalid_dispute`.
  1. create a `rng` from `rand_chacha::ChaChaRng::from_seed(compute_entropy::<T>(parent_hash))`.
  1. Invoke `limit_disputes` with the max block weight and `rng`, storing the returned weigh in `remaining_weight`.
  1. Fill up the remaining of the block weight with backed candidates and bitfields by invoking `apply_weight_limit` with `remaining_weigh` and `rng`.
  1. Return `Some(ParachainsInherentData { bitfields, backed_candidates, disputes, parent_header }`.
