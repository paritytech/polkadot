# Shared Module

This module is responsible for managing shared storage and configuration for other modules.

It is important that other pallets are able to use the Shared Module, so it should not have a
dependency on any other modules in the Parachains Runtime.

For the moment, it is used exclusively to track the current session index across the Parachains
Runtime system, and when it should be allowed to schedule future changes to Paras or Configurations.

## Constants

```rust
// `SESSION_DELAY` is used to delay any changes to Paras registration or configurations.
// Wait until the session index is 2 larger then the current index to apply any changes,
// which guarantees that at least one full session has passed before any changes are applied.
pub(crate) const SESSION_DELAY: SessionIndex = 2;
```

## Storage

```rust
/// The current session index within the Parachains Runtime system.
CurrentSessionIndex: SessionIndex;
/// All the validators actively participating in parachain consensus.
/// Indices are into the broader validator set.
ActiveValidatorIndices: Vec<ValidatorIndex>,
/// The parachain attestation keys of the validators actively participating in parachain consensus.
/// This should be the same length as `ActiveValidatorIndices`.
ActiveValidatorKeys: Vec<ValidatorId>
```

## Initialization

The Shared Module currently has no initialization routines.

The Shared Module is initialized directly after the Configuration module, but before all other
modules. It is important to update the Shared Module before any other module since its state may be
used within the logic of other modules, and it is important that the state is consistent across
them.

## Session Change

During a session change, the Shared Module receives and stores the current Session Index directly from the initializer module, along with the broader validator set, and it returns the new list of validators.

The list of validators should be first shuffled according to the chain's random seed and then truncated. The indices of these validators should be set to `ActiveValidatorIndices` and then returned back to the initializer. `ActiveValidatorKeys` should be set accordingly.

This information is used in the:

* Configuration Module: For delaying updates to configurations until at lease one full session has
  passed.
* Paras Module: For delaying updates to paras until at least one full session has passed.

## Finalization

The Shared Module currently has no finalization routines.

## Functions

* `scheduled_sessions() -> SessionIndex`: Return the next session index where updates to the
  Parachains Runtime system would be safe to apply.
* `set_session_index(SessionIndex)`: For tests. Set the current session index in the Shared Module.
