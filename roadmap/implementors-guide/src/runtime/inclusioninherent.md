# InclusionInherent

This module is responsible for all the logic carried by the `Inclusion` entry-point. This entry-point is mandatory, in that it must be invoked exactly once within every block, and it is also "inherent", in that it is provided with no origin by the block author. The data within it carries its own authentication. If any of the steps within fails, the entry-point is considered as having failed and the block will be invalid.

This module does not have the same initialization/finalization concerns as the others, as it only requires that entry points be triggered after all modules have initialized and that finalization happens after entry points are triggered. Both of these are assumptions we have already made about the runtime's order of operations, so this module doesn't need to be initialized or finalized by the `Initializer`.

## Storage

```rust
Included: Option<()>,
```

## Finalization

1. Take (get and clear) the value of `Included`. If it is not `Some`, throw an unrecoverable error.

## Entry Points

* `inclusion`: This entry-point accepts two parameters: [`Bitfields`](../types/availability.html#signed-availability-bitfield) and [`BackedCandidates`](../type-definitions.html#backed-candidate).
    1. The `Bitfields` are first forwarded to the `Inclusion::process_bitfields` routine, returning a set of freed cores. Provide a `Scheduler::core_para` as a core-lookup to the `process_bitfields` routine. Annotate each of these freed cores with `FreedReason::Concluded`.
    1. If `Scheduler::availability_timeout_predicate` is `Some`, invoke `Inclusion::collect_pending` using it, and add timed-out cores to the free cores, annotated with `FreedReason::TimedOut`.
    1. Invoke `Scheduler::schedule(freed)`
	1. Invoke the `Inclusion::process_candidates` routine with the parameters `(backed_candidates, Scheduler::scheduled(), Scheduler::group_validators)`.
    1. Call `Scheduler::occupied` using the return value of the `Inclusion::process_candidates` call above, first sorting the list of assigned core indices.
    1. If all of the above succeeds, set `Included` to `Some(())`.
