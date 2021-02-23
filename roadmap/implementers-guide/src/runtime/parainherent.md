# ParaInherent

This module is responsible for providing all data given to the runtime by the block author to the various parachains modules. The entry-point is mandatory, in that it must be invoked exactly once within every block, and it is also "inherent", in that it is provided with no origin by the block author. The data within it carries its own authentication; i.e. the data takes the form of signed statements by validators. If any of the steps within fails, the entry-point is considered as having failed and the block will be invalid.

This module does not have the same initialization/finalization concerns as the others, as it only requires that entry points be triggered after all modules have initialized and that finalization happens after entry points are triggered. Both of these are assumptions we have already made about the runtime's order of operations, so this module doesn't need to be initialized or finalized by the `Initializer`.

There are a couple of important notes to the operations in this inherent as they relate to disputes.
1. We don't accept bitfields or backed candidates if in "governance-only" mode from having a local dispute conclude on this fork.
1. When disputes are initiated, we remove the block from pending availability. This allows us to roll back chains to the block before blocks are included as opposed to backing. It's important to do this before processing bitfields.
1. `Inclusion::collect_disputed` is kind of expensive so it's important to gate this on whether there are actually any new disputes. Which should be never.
1. And we don't accept parablocks that have open disputes or disputes that have concluded against the candidate. It's important to import dispute statements before backing, but this is already the case as disputes are imported before processing bitfields.

## Storage

```rust
Included: Option<()>,
```

## Finalization

1. Take (get and clear) the value of `Included`. If it is not `Some`, throw an unrecoverable error.

## Entry Points

* `enter`: This entry-point accepts three parameters: The relay-chain parent block header, [`Bitfields`](../types/availability.md#signed-availability-bitfield) and [`BackedCandidates`](../types/backing.md#backed-candidate).
    1. Hash the parent header and make sure that it corresponds to the block hash of the parent (tracked by the `frame_system` FRAME module),
    1. Invoke `Disputes::provide_multi_dispute_data`.
    1. If `Disputes::is_frozen`, return and set `Included` to `Some(())`.
    1. If there are any created disputes from the current session, invoke `Inclusion::collect_disputed` with the disputed candidates. Annotate each returned core with `FreedReason::Concluded`.
    1. The `Bitfields` are first forwarded to the `Inclusion::process_bitfields` routine, returning a set of freed cores. Provide the number of availability cores (`Scheduler::availability_cores().len()`) as the expected number of bits and a `Scheduler::core_para` as a core-lookup to the `process_bitfields` routine. Annotate each of these freed cores with `FreedReason::Concluded`.
    1. For each freed candidate from the `Inclusion::process_bitfields` call, invoke `Disputes::note_included(current_session, candidate)`.
    1. If `Scheduler::availability_timeout_predicate` is `Some`, invoke `Inclusion::collect_pending` using it and annotate each of those freed cores with `FreedReason::TimedOut`.
    1. Combine and sort the dispute-freed cores, the bitfield-freed cores, and the timed-out cores.
    1. Invoke `Scheduler::clear`
    1. Invoke `Scheduler::schedule(freed_cores, System::current_block())`
    1. Extract `parent_storage_root` from the parent header,
    1. If `Disputes::could_be_invalid(current_session, candidate)` is true for any of the `backed_candidates`, fail.
    1. Invoke the `Inclusion::process_candidates` routine with the parameters `(parent_storage_root, backed_candidates, Scheduler::scheduled(), Scheduler::group_validators)`.
    1. Call `Scheduler::occupied` using the return value of the `Inclusion::process_candidates` call above, first sorting the list of assigned core indices.
    1. Call the `Ump::process_pending_upward_messages` routine to execute all messages in upward dispatch queues.
    1. If all of the above succeeds, set `Included` to `Some(())`.
