# Validation Code

Fetch the validation code (and its hash) used by a para, making the given `OccupiedCoreAssumption`.

```rust
fn validation_code(at: Block, ParaId, OccupiedCoreAssumption) -> Option<ValidationCodeAndHash>;
```
