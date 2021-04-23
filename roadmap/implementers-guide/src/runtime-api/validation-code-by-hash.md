# Validation Code By Hash

Fetch the current validation code, future validation code or past validation code (if not pruned) used by a para from the hash of the validation code.

Past validation code of para are pruned as configured in runtime configuration module.

```rust
fn validation_code_by_hash(at: Block, Hash) -> Option<ValidationCode>;
```
