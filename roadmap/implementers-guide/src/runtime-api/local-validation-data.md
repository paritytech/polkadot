# Local Validation Data

Yields the [`LocalValidationData`](../types/candidate.md#localvalidationdata) for the given [`ParaId`](../types/candidate.md#paraid) along with an assumption that should be used if the para currently occupies a core: whether the candidate occupying that core should be assumed to have been made available and included or timed out and discarded, along with a third option to assert that the core was not occupied. This choice affects everything from the parent head-data, the validation code, and the state of message-queues. Typically, users will take the assumption that either the core was free or that the occupying candidate was included, as timeouts are expected only in adversarial circumstances and even so, only in a small minority of blocks directly following validator set rotations.

The documentation of [`LocalValidationData`](../types/candidate.md#localvalidationdata) has more information on this dichotomy.

```rust
/// An assumption being made about the state of an occupied core.
enum OccupiedCoreAssumption {
    /// The candidate occupying the core was made available and included to free the core.
    Included,
    /// The candidate occupying the core timed out and freed the core without advancing the para.
    TimedOut,
    /// The core was not occupied to begin with.
    Free,
}

/// Returns the local validation data for the given para and occupied core assumption.
///
/// Returns `None` if either the para is not registered or the assumption is `Freed`
/// and the para already occupies a core.
fn local_validation_data(at: Block, ParaId, OccupiedCoreAssumption) -> Option<LocalValidationData>;
```
