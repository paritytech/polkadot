# ApprovalsInherent Module

This module is responsible for all the logic carried by the `Approvals` entry-point. This entry-point is mandatory, in that it must be invoked exactly once within every block, and it is also "inherent", in that it is provided with no origin by the block author. The data within it carries its own authentication. If any of the steps within fails, the entry-point is considered as having failed and the block will be invalid.

This module does not have the same initialization/finalization concerns as the others, as it only requires that entry points be triggered after all modules have initialized and that finalization happens after entry points are triggered. Both of these are assumptions we have already made about the runtime's order of operations, so this module doesn't need to be initialized or finalized by the `Initializer`.

## Storage

```rust
Included: Option<()>,
```

## Finalization

1. Take (get and clear) the value of `Included`. If it is not `Some`, throw an unrecoverable error.

## Entry Points

* `assignments_approvals`: This entry-point accepts two parameters: [`SubmittedAssignments`](../types/approval.md#submittedassignments) and [`SubmittedApprovals`](../types/approval.md#submittedapprovals).
    1. Call [`Approvals::submit_assignments](approvals.md)` with the submitted assignments.
    1. Call [`Approvals::submit_approvals](approvals.md)` with the submitted approvals.
    1. If all of the above succeeds, set `Included` to `Some(())`.
