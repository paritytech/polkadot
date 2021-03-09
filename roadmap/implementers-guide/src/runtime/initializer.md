# Initializer Module

This module is responsible for initializing the other modules in a deterministic order. It also has one other purpose as described in the overview of the runtime: accepting and forwarding session change notifications.

## Storage

```rust
HasInitialized: bool;
// buffered session changes along with the block number at which they should be applied.
//
// typically this will be empty or one element long. ordered ascending by BlockNumber and insertion
// order.
BufferedSessionChanges: Vec<(BlockNumber, ValidatorSet, ValidatorSet)>;
```

## Initialization

Before initializing modules, remove all changes from the `BufferedSessionChanges` with number less than or equal to the current block number, and apply the last one. The session change is applied to all modules in the same order as initialization.

The other parachains modules are initialized in this order:

1. Configuration
1. Paras
1. Scheduler
1. Inclusion
1. SessionInfo
1. Disputes
1. DMP
1. UMP
1. HRMP

The [Configuration Module](configuration.md) is first, since all other modules need to operate under the same configuration as each other. It would lead to inconsistency if, for example, the scheduler ran first and then the configuration was updated before the Inclusion module.

Set `HasInitialized` to true.

## Session Change

Store the session change information in `BufferedSessionChange` along with the block number at which it was submitted, plus one. Although the expected operational parameters of the block authorship system should prevent more than one change from being buffered at any time, it may occur. Regardless, we always need to track the block number at which the session change can be applied so as to remain flexible over session change notifications being issued before or after initialization of the current block.

## Finalization

Finalization order is less important in this case than initialization order, so we finalize the modules in the reverse order from initialization.

Set `HasInitialized` to false.
