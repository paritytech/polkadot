# Validation Code

Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.

```rust
fn validation_code(at: Block, ParaId, OccupiedCoreAssumption) -> Option<ValidationCode>;
```

Fetch the validation code (past, present or future) by its hash.

```rust
fn validation_code_by_hash(at: Block, ValidationCodeHash) -> Option<ValidationCode>;
```

Fetch the validation code hash used by a para, making the given `OccupiedCoreAssumption`.

> ⚠️ This API was introduced in `ParachainHost` v2.

```rust
fn validation_code_hash(at: Block, ParaId, OccupiedCoreAssumption) -> Option<ValidationCodeHash>;
```
