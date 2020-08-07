# Global Validation Data

Yields the [`GlobalValidationData`](../types/candidate.md#globalvalidationschedule) at the state of a given block. This applies to all para candidates with the relay-parent equal to that block.

```rust
fn global_validation_data(at: Block) -> GlobalValidationData;
```
