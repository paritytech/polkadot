# Configuration Module

This module is responsible for managing all configuration of the parachain host in-flight. It provides a central point for configuration updates to prevent races between configuration changes and parachain-processing logic. Configuration can only change during the session change routine, and as this module handles the session change notification first it provides an invariant that the configuration does not change throughout the entire session. Both the [scheduler](scheduler.md) and [inclusion](inclusion.md) modules rely on this invariant to ensure proper behavior of the scheduler.

The configuration that we will be tracking is the [`HostConfiguration`](../types/runtime.md#host-configuration) struct.

## Storage

The configuration module is responsible for two main pieces of storage.

```rust
/// The current configuration to be used.
Configuration: HostConfiguration;
/// A pending configuration to be applied on session change.
PendingConfigs: Vec<(SessionIndex, HostConfiguration)>;
/// A flag that says if the consistency checks should be omitted.
BypassConsistencyCheck: bool;
```

## Session change

The session change routine works as follows:

- If there is no pending configurations, then return early.
- Take all pending configurations that are less than or equal to the current session index.
  - Get the pending configuration with the highest session index and apply it to the current configuration. Discard the earlier ones if any.

## Routines

```rust
enum InconsistentError {
  // ...
}

impl HostConfiguration {
  fn check_consistency(&self) -> Result<(), InconsistentError> { /* ... */ }
}

/// Get the host configuration.
pub fn configuration() -> HostConfiguration {
  Configuration::get()
}

/// Schedules updating the host configuration. The update is given by the `updater` closure. The 
/// closure takes the current version of the configuration and returns the new version. 
/// Returns an `Err` if the closure returns a broken configuration. However, there are a couple of 
/// exceptions: 
///
/// - if the configuration that was passed in the closure is already broken, then it will pass the 
/// update: you cannot break something that is already broken.
/// - If the `BypassConsistencyCheck` flag is set, then the checks will be skipped.
///
/// The changes made by this function will always be scheduled at session X, where X is the current session index + 2.
/// If there is already a pending update for X, then the closure will receive the already pending configuration for 
/// session X.
///
/// If there is already a pending update for the current session index + 1, then it won't be touched. Otherwise,
/// that would violate the promise of this function that changes will be applied on the second session change (cur + 2).
fn schedule_config_update(updater: impl FnOnce(&mut HostConfiguration<T::BlockNumber>)) -> DispatchResult
```

## Entry-points

The Configuration module exposes an entry point for each configuration member. These entry-points accept calls only from governance origins. These entry-points will use the `update_configuration` routine to update the specific configuration field.
