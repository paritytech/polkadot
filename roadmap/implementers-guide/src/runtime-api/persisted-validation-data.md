# Persisted Validation Data

Yields the [`PersistedValidationData`](../types/candidate.md#persistedvalidationdata) for the given [`ParaId`](../types/candidate.md#paraid) along with an assumption that should be used if the para currently occupies a core:

```rust
/// Returns the persisted validation data for the given para and occupied core assumption.
///
/// Returns `None` if either the para is not registered or the assumption is `Freed`
/// and the para already occupies a core.
fn persisted_validation_data(at: Block, ParaId, OccupiedCoreAssumption) -> Option<PersistedValidationData>;
```