# DisputeInherent Module

This module propagates remote dispute data into the runtime. This entrypoint is mandatory, in that
it must be invoked exactly once within every block, and it is also inherent in that it is provided
with no origin by the block author. The data wihtin it carries its own authentication. If any of
the steps within fails, the entrypoint is considered to have failed and the block will be invalid.

## Storage

```rust
Included: Option<()>,
```

## Finalization

1. Take (get and clear) the value of `Included`. Ensure that it is `None` or throw an unrecoverable error.

## Entry Points

- `disputes`.

  > TODO: design how the disputes inherent is used to propagate unconcluded remote disputes
