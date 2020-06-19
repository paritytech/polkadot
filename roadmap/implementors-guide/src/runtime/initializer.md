# Initializer Module

This module is responsible for initializing the other modules in a deterministic order. It also has one other purpose as described above: accepting and forwarding session change notifications.

## Storage

```rust
HasInitialized: bool
```

## Initialization

The other modules are initialized in this order:

1. Configuration
1. Paras
1. Scheduler
1. Inclusion
1. Validity.
1. Router.

The [Configuration Module](configuration.html) is first, since all other modules need to operate under the same configuration as each other. It would lead to inconsistency if, for example, the scheduler ran first and then the configuration was updated before the Inclusion module.

Set `HasInitialized` to true.

## Session Change

If `HasInitialized` is true, throw an unrecoverable error (panic).
Otherwise, forward the session change notification to other modules in initialization order.

## Finalization

Finalization order is less important in this case than initialization order, so we finalize the modules in the reverse order from initialization.

Set `HasInitialized` to false.
