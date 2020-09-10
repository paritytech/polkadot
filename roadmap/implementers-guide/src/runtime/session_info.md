# Session Info

For disputes and approvals, we need access to information about validator sets from prior sessions. We also often want easy access to the same information about the current session's validator set. This module aggregates and stores this information in a rolling window while providing easy APIs for access.

## Storage

Helper structs:

```rust
struct SessionInfo {
    // validators in canonical ordering.
    validators: Vec<ValidatorId>,
    // validators' authority discovery keys for the session in canonical ordering.
    discovery_keys: Vec<DiscoveryId>,
    // The assignment and approval keys for validators.
    approval_keys: Vec<(AssignmentId, ApprovalId)>,
    // validators in shuffled ordering
    validator_groups: Vec<Vec<ValidatorIndex>>,
    // the zeroth delay tranche width.
    zeroth_delay_tranche_width: u32,
    // The number of samples we do of relay_vrf_modulo.
    relay_vrf_modulo_samples: u32,
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
/// Previous session information. Should have an entry from `EarliestStoredSession..CurrentSessionIndex`
PrevSessions: map SessionIndex => Option<SessionInfo>,
/// Current session info.
CurrentSessionInfo: SessionInfo,
```

## Session Change

1. Update the `CurrentSessionIndex`.
1. Update `EarliestStoredSession` based on `config.dispute_period` and remove all entries from `PrevSessions` from the previous value up to the new value.
1. Create a new entry in `PrevSessions` using the old value of `CurrentSessionInfo`, which should be replaced by updated information about the current session.

## Routines

* `earliest_stored_session() -> SessionIndex`: Yields the earliest session for which we have information stored.
* `session_info(session: SessionIndex) -> Option<SessionInfo>`: Yields the session info for the given session, if stored.