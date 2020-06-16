# Configuration Module

This module is responsible for managing all configuration of the parachain host in-flight. It provides a central point for configuration updates to prevent races between configuration changes and parachain-processing logic. Configuration can only change during the session change routine, and as this module handles the session change notification first it provides an invariant that the configuration does not change throughout the entire session. Both the [scheduler](scheduler.html) and [inclusion](inclusion.html) modules rely on this invariant to ensure proper behavior of the scheduler.

The configuration that we will be tracking is the [`HostConfiguration`](../types/runtime.html#host-configuration) struct.

## Storage

The configuration module is responsible for two main pieces of storage.

```rust
/// The current configuration to be used.
Configuration: HostConfiguration;
/// A pending configuration to be applied on session change.
PendingConfiguration: Option<HostConfiguration>;
```

## Session change

The session change routine for the Configuration module is simple. If the `PendingConfiguration` is `Some`, take its value and set `Configuration` to be equal to it. Reset `PendingConfiguration` to `None`.

## Routines

```rust
/// Get the host configuration.
pub fn configuration() -> HostConfiguration {
  Configuration::get()
}

/// Updating the pending configuration to be applied later.
fn update_configuration(f: impl FnOnce(&mut HostConfiguration)) {
  PendingConfiguration::mutate(|pending| {
    let mut x = pending.unwrap_or_else(Self::configuration);
    f(&mut x);
    *pending = Some(x);
  })
}
```

## Entry-points

The Configuration module exposes an entry point for each configuration member. These entry-points accept calls only from governance origins. These entry-points will use the `update_configuration` routine to update the specific configuration field.
