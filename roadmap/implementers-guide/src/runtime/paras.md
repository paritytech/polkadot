# Paras Module

The Paras module is responsible for storing information on parachains and parathreads. Registered
parachains and parathreads cannot change except at session boundaries and after at least a full
session has passed. This is primarily to ensure that the number and meaning of bits required for the
availability bitfields does not change except at session boundaries.

It's also responsible for:

- managing parachain validation code upgrades as well as maintaining availability of old parachain 
code and its pruning.
- vetting PVFs by means of the PVF pre-checking mechanism.

## Storage

### Utility Structs

```rust
// the two key times necessary to track for every code replacement.
pub struct ReplacementTimes {
 /// The relay-chain block number that the code upgrade was expected to be activated.
 /// This is when the code change occurs from the para's perspective - after the
 /// first parablock included with a relay-parent with number >= this value.
 expected_at: BlockNumber,
 /// The relay-chain block number at which the parablock activating the code upgrade was
 /// actually included. This means considered included and available, so this is the time at which
 /// that parablock enters the acceptance period in this fork of the relay-chain.
 activated_at: BlockNumber,
}

/// Metadata used to track previous parachain validation code that we keep in
/// the state.
pub struct ParaPastCodeMeta {
 // Block numbers where the code was expected to be replaced and where the code
 // was actually replaced, respectively. The first is used to do accurate lookups
 // of historic code in historic contexts, whereas the second is used to do
 // pruning on an accurate timeframe. These can be used as indices
 // into the `PastCode` map along with the `ParaId` to fetch the code itself.
 upgrade_times: Vec<ReplacementTimes>,
 // This tracks the highest pruned code-replacement, if any.
 last_pruned: Option<BlockNumber>,
}

struct ParaGenesisArgs {
  /// The initial head-data to use.
  genesis_head: HeadData,
  /// The validation code to start with.
  validation_code: ValidationCode,
  /// True if parachain, false if parathread.
  parachain: bool,
}

/// The possible states of a para, to take into account delayed lifecycle changes.
pub enum ParaLifecycle {
  /// A Para is new and is onboarding.
  Onboarding,
  /// Para is a Parathread.
  Parathread,
  /// Para is a Parachain.
  Parachain,
  /// Para is a Parathread which is upgrading to a Parachain.
  UpgradingParathread,
  /// Para is a Parachain which is downgrading to a Parathread.
  DowngradingParachain,
  /// Parathread is being offboarded.
  OutgoingParathread,
  /// Parachain is being offboarded.
  OutgoingParachain,
}

enum PvfCheckCause {
  /// PVF vote was initiated by the initial onboarding process of the given para.
  Onboarding(ParaId),
  /// PVF vote was initiated by signalling of an upgrade by the given para.
  Upgrade {
    /// The ID of the parachain that initiated or is waiting for the conclusion of pre-checking.
    id: ParaId,
    /// The relay-chain block number that was used as the relay-parent for the parablock that
    /// initiated the upgrade.
    relay_parent_number: BlockNumber,
  },
}

struct PvfCheckActiveVoteState {
  // The two following vectors have their length equal to the number of validators in the active
  // set. They start with all zeroes. A 1 is set at an index when the validator at the that index
  // makes a vote. Once a 1 is set for either of the vectors, that validator cannot vote anymore.
  // Since the active validator set changes each session, the bit vectors are reinitialized as
  // well: zeroed and resized so that each validator gets its own bit.
  votes_accept: BitVec,
  votes_reject: BitVec,

  /// The number of session changes this PVF vote has observed. Therefore, this number is
  /// increased at each session boundary. When created, it is initialized with 0.
  age: SessionIndex,
  /// The block number at which this PVF vote was created.
  created_at: BlockNumber,
  /// A list of causes for this PVF pre-checking. Has at least one.
  causes: Vec<PvfCheckCause>,
}
```

#### Para Lifecycle

Because the state changes of parachains and parathreads are delayed, we track the specific state of 
the para using the `ParaLifecycle` enum.

```
None                 Parathread                  Parachain
 +                        +                          +
 |                        |                          |
 |   (≈2 Session Delay)   |                          |
 |                        |                          |
 +----------------------->+                          |
 |       Onboarding       |                          |
 |                        |                          |
 +-------------------------------------------------->+
 |       Onboarding       |                          |
 |                        |                          |
 |                        +------------------------->+
 |                        |   UpgradingParathread    |
 |                        |                          |
 |                        +<-------------------------+
 |                        |   DowngradingParachain   |
 |                        |                          |
 |<-----------------------+                          |
 |   OutgoingParathread   |                          |
 |                        |                          |
 +<--------------------------------------------------+
 |                        |    OutgoingParachain     |
 |                        |                          |
 +                        +                          +
```

Note that if PVF pre-checking is enabled, onboarding of a para may potentially be delayed. This can
happen due to PVF pre-checking voting concluding late.

During the transition period, the para object is still considered in its existing state.

### Storage Layout

```rust
/// All currently active PVF pre-checking votes.
///
/// Invariant:
/// - There are no PVF pre-checking votes that exists in list but not in the set and vice versa.
PvfActiveVoteMap: map ValidationCodeHash => PvfCheckActiveVoteState;
/// The list of all currently active PVF votes. Auxiliary to `PvfActiveVoteMap`.
PvfActiveVoteList: Vec<ValidationCodeHash>;
/// All parachains. Ordered ascending by ParaId. Parathreads are not included.
Parachains: Vec<ParaId>,
/// The current lifecycle state of all known Para Ids.
ParaLifecycle: map ParaId => Option<ParaLifecycle>,
/// The head-data of every registered para.
Heads: map ParaId => Option<HeadData>;
/// The validation code hash of every live para.
CurrentCodeHash: map ParaId => Option<ValidationCodeHash>;
/// Actual past code hash, indicated by the para id as well as the block number at which it became outdated.
PastCodeHash: map (ParaId, BlockNumber) => Option<ValidationCodeHash>;
/// Past code of parachains. The parachains themselves may not be registered anymore,
/// but we also keep their code on-chain for the same amount of time as outdated code
/// to keep it available for secondary checkers.
PastCodeMeta: map ParaId => ParaPastCodeMeta;
/// Which paras have past code that needs pruning and the relay-chain block at which the code was replaced.
/// Note that this is the actual height of the included block, not the expected height at which the
/// code upgrade would be applied, although they may be equal.
/// This is to ensure the entire acceptance period is covered, not an offset acceptance period starting
/// from the time at which the parachain perceives a code upgrade as having occurred.
/// Multiple entries for a single para are permitted. Ordered ascending by block number.
PastCodePruning: Vec<(ParaId, BlockNumber)>;
/// The block number at which the planned code change is expected for a para.
/// The change will be applied after the first parablock for this ID included which executes
/// in the context of a relay chain block with a number >= `expected_at`.
FutureCodeUpgrades: map ParaId => Option<BlockNumber>;
/// Hash of the actual future code of a para.
FutureCodeHash: map ParaId => Option<ValidationCodeHash>;
/// This is used by the relay-chain to communicate to a parachain a go-ahead with in the upgrade procedure.
///
/// This value is absent when there are no upgrades scheduled or during the time the relay chain
/// performs the checks. It is set at the first relay-chain block when the corresponding parachain
/// can switch its upgrade function. As soon as the parachain's block is included, the value
/// gets reset to `None`.
///
/// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
/// the format will require migration of parachains.
UpgradeGoAheadSignal: map hasher(twox_64_concat) ParaId => Option<UpgradeGoAhead>;
/// This is used by the relay-chain to communicate that there are restrictions for performing
/// an upgrade for this parachain.
///
/// This may be a because the parachain waits for the upgrade cooldown to expire. Another
/// potential use case is when we want to perform some maintanance (such as storage migration)
/// we could restrict upgrades to make the process simpler.
///
/// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
/// the format will require migration of parachains.
UpgradeRestrictionSignal: map hasher(twox_64_concat) ParaId => Option<UpgradeRestriction>;
/// The list of parachains that are awaiting for their upgrade restriction to cooldown.
///
/// Ordered ascending by block number.
UpgradeCooldowns: Vec<(ParaId, T::BlockNumber)>;
/// The list of upcoming code upgrades. Each item is a pair of which para performs a code
/// upgrade and at which relay-chain block it is expected at.
///
/// Ordered ascending by block number.
UpcomingUpgrades: Vec<(ParaId, T::BlockNumber)>;
/// The actions to perform during the start of a specific session index.
ActionsQueue: map SessionIndex => Vec<ParaId>;
/// Upcoming paras instantiation arguments.
///
/// NOTE that after PVF pre-checking is enabled the para genesis arg will have it's code set 
/// to empty. Instead, the code will be saved into the storage right away via `CodeByHash`.
UpcomingParasGenesis: map ParaId => Option<ParaGenesisArgs>;
/// The number of references on the validation code in `CodeByHash` storage.
CodeByHashRefs: map ValidationCodeHash => u32;
/// Validation code stored by its hash.
CodeByHash: map ValidationCodeHash => Option<ValidationCode>
```

## Session Change

1. Execute all queued actions for paralifecycle changes:
  1. Clean up outgoing paras.
     1. This means removing the entries under `Heads`, `CurrentCode`, `FutureCodeUpgrades`, and
        `FutureCode`. An according entry should be added to `PastCode`, `PastCodeMeta`, and
        `PastCodePruning` using the outgoing `ParaId` and removed `CurrentCode` value. This is
        because any outdated validation code must remain available on-chain for a determined amount
        of blocks, and validation code outdated by de-registering the para is still subject to that
        invariant.
  1. Apply all incoming paras by initializing the `Heads` and `CurrentCode` using the genesis
     parameters.
  1. Amend the `Parachains` list and `ParaLifecycle` to reflect changes in registered parachains.
  1. Amend the `ParaLifecycle` set to reflect changes in registered parathreads.
  1. Upgrade all parathreads that should become parachains, updating the `Parachains` list and
     `ParaLifecycle`.
  1. Downgrade all parachains that should become parathreads, updating the `Parachains` list and
     `ParaLifecycle`.
  1. (Deferred) Return list of outgoing paras to the initializer for use by other modules.
1. Go over all active PVF pre-checking votes:
  1. Increment `age` of the vote.
  1. If `age` reached `cfg.pvf_voting_ttl`, then enact PVF rejection and remove the vote from the active list.
  1. Otherwise, reinitialize the ballots.
    1. Resize the `votes_accept`/`votes_reject` to have the same length as the incoming validator set.
    1. Zero all the votes.
## Initialization

1. Do pruning based on all entries in `PastCodePruning` with `BlockNumber <= now`. Update the
   corresponding `PastCodeMeta` and `PastCode` accordingly.
1. Toggle the upgrade related signals
  1. Collect all `(para_id, expected_at)` from `UpcomingUpgrades` where `expected_at <= now` and prune them. For each para pruned set `UpgradeGoAheadSignal` to `GoAhead`. Reserve weight for the state modification to upgrade each para pruned.
  1. Collect all `(para_id, next_possible_upgrade_at)` from `UpgradeCooldowns` where `next_possible_upgrade_at <= now`. For each para obtained this way reserve weight to remove its `UpgradeRestrictionSignal` on finalization.

## Routines

* `schedule_para_initialize(ParaId, ParaGenesisArgs)`: Schedule a para to be initialized at the next
  session. Noop if para is already registered in the system with some `ParaLifecycle`.
* `schedule_para_cleanup(ParaId)`: Schedule a para to be cleaned up after the next full session.
* `schedule_parathread_upgrade(ParaId)`: Schedule a parathread to be upgraded to a parachain.
* `schedule_parachain_downgrade(ParaId)`: Schedule a parachain to be downgraded to a parathread.
* `schedule_code_upgrade(ParaId, new_code, relay_parent: BlockNumber, HostConfiguration)`: Schedule a future code
  upgrade of the given parachain. In case the PVF pre-checking is disabled, or the new code is already present in the storage, the upgrade will be applied after inclusion of a block of the same parachain
  executed in the context of a relay-chain block with number >= `relay_parent + config.validation_upgrade_delay`. If the upgrade is scheduled `UpgradeRestrictionSignal` is set and it will remain set until `relay_parent + config.validation_upgrade_cooldown`.
In case the PVF pre-checking is enabled, or the new code is not already present in the storage, then the PVF pre-checking run will be scheduled for that validation code. If the pre-checking concludes with rejection, then the upgrade is canceled. Otherwise, after pre-checking is concluded the upgrade will be scheduled and be enacted as described above.
* `note_new_head(ParaId, HeadData, BlockNumber)`: note that a para has progressed to a new head,
  where the new head was executed in the context of a relay-chain block with given number. This will
  apply pending code upgrades based on the block number provided. If an upgrade took place it will clear the `UpgradeGoAheadSignal`.
* `lifecycle(ParaId) -> Option<ParaLifecycle>`: Return the `ParaLifecycle` of a para.
* `is_parachain(ParaId) -> bool`: Returns true if the para ID references any live parachain,
  including those which may be transitioning to a parathread in the future.
* `is_parathread(ParaId) -> bool`: Returns true if the para ID references any live parathread,
  including those which may be transitioning to a parachain in the future.
* `is_valid_para(ParaId) -> bool`: Returns true if the para ID references either a live parathread
  or live parachain.
* `can_upgrade_validation_code(ParaId) -> bool`: Returns true if the given para can signal code upgrade right now.
* `pvfs_require_prechecking() -> Vec<ValidationCodeHash>`: Returns the list of PVF validation code hashes that require PVF pre-checking votes.

## Finalization

Collect all `(para_id, next_possible_upgrade_at)` from `UpgradeCooldowns` where `next_possible_upgrade_at <= now` and prune them. For each para pruned remove its `UpgradeRestrictionSignal`.
