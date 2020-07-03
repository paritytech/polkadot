# Initializer Module

This module is responsible for initializing the other modules in a deterministic order. It also has one other purpose as described in the overview of the runtime: accepting and forwarding session change notifications.

## Storage

```rust
HasInitialized: bool;
BufferedSessionChange: Option<(ValidatorSet, ValidatorSet)>; // (new, queued)
```

## Initialization

Before initializing modules, we apply the `BufferedSessionChange`, if any, and remove it from storage. The session change is applied to all modules in the same order as initialization.

The other parachains modules are initialized in this order:

1. Configuration
1. Paras
1. Scheduler
1. Inclusion
1. Validity.
1. Router.

The [Configuration Module](configuration.md) is first, since all other modules need to operate under the same configuration as each other. It would lead to inconsistency if, for example, the scheduler ran first and then the configuration was updated before the Inclusion module.

Set `HasInitialized` to true.

## Session Change

Store the session change information in `BufferedSessionChange`. If there is already a value present, it should be overwritten. The only way there can already be a value present is if either 2 session changes occur within one block, or one session-change happens after initialization and then another happens before initialization in the next block. In either case, although both are far outside of the expected operational parameters of the chain, there is no time occupied where the clobbered validator set can reasonably be in charge of any validation work, so we do not lose anything by clobbering it.

## Finalization

Finalization order is less important in this case than initialization order, so we finalize the modules in the reverse order from initialization.

Set `HasInitialized` to false.
