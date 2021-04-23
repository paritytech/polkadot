# Validation Code Hash

Fetch the hash of the validation code used by a para, making the given `OccupiedCoreAssumption`.

```rust
fn validation_code_hash(at: Block, ParaId, OccupiedCoreAssumption) -> Option<Hash>;
```

The code can be fetched from the hash using `validation_code_by_hash` function.
