# Validation Code

Fetch the validation code (and its hash) used by a para, making the given `OccupiedCoreAssumption`.

```rust
fn validation_code(at: Block, ParaId, OccupiedCoreAssumption) -> Option<ValidationCodeAndHash>;
```

Fetch the validation code (past, present or future) by its hash.

```rust
fn validation_code_by_hash(at: Block, ValidationCodeHash) -> Option<ValidationCode>;
```
