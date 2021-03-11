# Session Info

For disputes and approvals, we need access to information about validator sets from prior sessions. We also often want easy access to the same information about the current session's validator set. This module aggregates and stores this information in a rolling window while providing easy APIs for access.

## Storage

Helper structs:

```rust
struct SessionInfo {
    // validators in canonical ordering. These are the public keys used for backing,
    // dispute participation, and approvals.
    validators: Vec<ValidatorId>,
    // validators' authority discovery keys for the session in canonical ordering.
    discovery_keys: Vec<DiscoveryId>,
    // The assignment keys for validators.
    assignment_keys: Vec<AssignmentId>,
    // validators in shuffled ordering - these are the validator groups as produced
    // by the `Scheduler` module for the session and are typically referred to by
    // `GroupIndex`.
    validator_groups: Vec<Vec<ValidatorIndex>>,
    // The number of availability cores used by the protocol during this session.
    n_cores: u32,
    // the zeroth delay tranche width.
    zeroth_delay_tranche_width: u32,
    // The number of samples we do of relay_vrf_modulo.
    relay_vrf_modulo_samples: u32,
    // The number of delay tranches in total.
    n_delay_tranches: u32,
    // How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
    // no-show.
    no_show_slots: u32,
    /// The number of validators needed to approve a block.
    needed_approvals: u32,
}
```

Storage Layout:

```rust
/// The earliest session for which previous session info is stored.
EarliestStoredSession: SessionIndex,
/// Session information. Should have an entry from `EarliestStoredSession..=CurrentSessionIndex`
Sessions: map SessionIndex => Option<SessionInfo>,
```

## Session Change

1. Update `EarliestStoredSession` based on `config.dispute_period` and remove all entries from `Sessions` from the previous value up to the new value.
1. Create a new entry in `Sessions` with information about the current session. Use `shared::ActiveValidators` to determine the indices into the broader validator sets (validation, assignment, discovery) which are actually used for parachain validation. Only these validators should appear in the `SessionInfo`.

## Routines

* `earliest_stored_session() -> SessionIndex`: Yields the earliest session for which we have information stored.
* `session_info(session: SessionIndex) -> Option<SessionInfo>`: Yields the session info for the given session, if stored.
