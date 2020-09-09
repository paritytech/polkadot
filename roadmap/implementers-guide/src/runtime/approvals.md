# Approvals Module

The approvals module implements the runtime component of the system described in the section on [Approvals](../protocol-approval.md).

This module is responsible for tracking included candidates in the current chain which still need to be approved, which validators have submitted announcements of their assignment for those blocks, the approval votes cast by those validators on those blocks, and the highest ancestor of the current chain which has been approved.

## Storage

Helper structs:

```rust
struct BlockEntry {
    // The result of `make_bytes` on the relay-chain block authorship VRF
    relay_vrf_bytes: VRFInOut,
    // The candidates included within this block. Ascending by core index.
    candidates: Vec<(Hash, CoreIndex)>,  
}

struct AssignedValidator {
    index: ValidatorIndex,
    // `None` indicates no opinion yet, `Some` is approved.
    approved: Option<ExecutionTimePair>,
    no_show: bool,
}

struct CandidateEntry {
    session: SessionIndex,
    candidate: CandidateDescriptor,
    availability_bitfield: Bitfield,
    core: CoreIndex,
    backing_group: GroupIndex,
    assigned_validators: Bitfield, // 1 for each validator.
    // outer vec denotes tranche number. inner vecs are sorted ascending by validator index.
    validator_approvals: Vec<Vec<AssignedValidator>>, 
    total_assigned: u32,
    approval_votes: u32,
}

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
}
```

Storage Layout:

```rust
/// The number of blocks in this chain which have been fully approved.
ApprovedHead: BlockNumber,
/// The unapproved suffix of the chain: 1 bit for each block after `ApprovedHead`.
///
/// An entry in the vec is 0 if the block has not had all candidates within it approved and 1 if it
/// has.
/// Since a relay-chain block is considered approved when it has all candidates within it approved
/// and its parent is approved, we can maintain the invariant that the first item of this vec, if
/// it exists, is always 0.
///
/// Whenever there is a prefix of `1`s to this vector, that prefix can be removed and the
/// `ApprovedHead` can be incremented by the length of the removed prefix.
UnapprovedSuffix: BitVec,
/// The current session index.
CurrentSessionIndex: SessionIndex,
/// Approval metadata entries for every block in the unapproved suffix.
BlockEntries: map BlockNumber => BlockEntry,
/// Approval metadata entries for every candidate in every block in the unapproved suffix.
CandidateEntries: double_map BlockNumber, Hash => CandidateEntry,

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

Helper Structs:

```rust
struct IncludedCandidate {
    candidate: CandidateDescriptor,
    backing_group: GroupIndex,
    availability_bitfield: Bitfield,
}
```

* `approve_included(included_on_cores: Vec<Option<IncludedCandidate>>)`:
  1. Create a `BlockEntry` for the current block number
  1. If the vector of `included_on_cores` is only `None`s, add a `1` to `UnapprovedSuffix`.
  1. If not, add a `0` to `UnapprovedSuffix`
  1. For every `Some` entry in `included_on_cores`, create a `CandidateEntry` and add an item to the `BlockEntry` for the candidate. The core index is the index of the item in the `included_on_cores` vector.
* `submit_assignments(assignments: SubmittedAssignments)`:
  1. Ensure each block number is unapproved.
  1. Ensure each core had a candidate leaving it at that block
  1. Ensure each assignment is valid according to the assignment criteria and compute the tranche
  1. Ensure each assigned validator was not already assigned to the candidate and appears only once in the submitted assignments.
  1. Ignore assignments from validators who were in the backing group.
  1. Update `assigned_validators` for each included validator.
* `submit_approvals(approvals: SubmittedApprovals)`:
  1. Ensure each block number is unapproved.
  1. Ensure each core had a candidate leaving it at that block.
  1. Check signatures on each approval vote.
  1. Update tallies assigned vs. approved
  1. If the flag indicates that the block has been fully approved, sweep through and check all candidates to see if all assigned validators have approved and there were enough approval votes. If so, note the block as approved in `UnapprovedSuffix`.
  1. Remove the 1-prefix of `UnapprovedSuffix`, removing all block entries and candidate entries associated.