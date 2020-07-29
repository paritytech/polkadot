# Paras Module

The Paras module is responsible for storing information on parachains and parathreads. Registered parachains and parathreads cannot change except at session boundaries. This is primarily to ensure that the number of bits required for the availability bitfields does not change except at session boundaries.

It's also responsible for managing parachain validation code upgrades as well as maintaining availability of old parachain code and its pruning.

## Storage

Utility structs:

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

enum UseCodeAt {
 // Use the current code.
 Current,
 // Use the code that was replaced at the given block number.
 ReplacedAt(BlockNumber),
}

struct ParaGenesisArgs {
  /// The initial head-data to use.
  genesis_head: HeadData,
  /// The validation code to start with.
  validation_code: ValidationCode,
  /// True if parachain, false if parathread.
  parachain: bool,
}
```

Storage layout:

```rust
/// All parachains. Ordered ascending by ParaId. Parathreads are not included.
Parachains: Vec<ParaId>,
/// All parathreads.
Parathreads: map ParaId => Option<()>,
/// The head-data of every registered para.
Heads: map ParaId => Option<HeadData>;
/// The validation code of every live para.
ValidationCode: map ParaId => Option<ValidationCode>;
/// Actual past code, indicated by the para id as well as the block number at which it became outdated.
PastCode: map (ParaId, BlockNumber) => Option<ValidationCode>;
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
/// The actual future code of a para.
FutureCode: map ParaId => Option<ValidationCode>;

/// Upcoming paras (chains and threads). These are only updated on session change. Corresponds to an
/// entry in the upcoming-genesis map.
UpcomingParas: Vec<ParaId>;
/// Upcoming paras instantiation arguments.
UpcomingParasGenesis: map ParaId => Option<ParaGenesisArgs>;
/// Paras that are to be cleaned up at the end of the session.
OutgoingParas: Vec<ParaId>;
```

## Session Change

1. Clean up outgoing paras. This means removing the entries under `Heads`, `ValidationCode`, `FutureCodeUpgrades`, and `FutureCode`. An according entry should be added to `PastCode`, `PastCodeMeta`, and `PastCodePruning` using the outgoing `ParaId` and removed `ValidationCode` value. This is because any outdated validation code must remain available on-chain for a determined amount of blocks, and validation code outdated by de-registering the para is still subject to that invariant.
1. Apply all incoming paras by initializing the `Heads` and `ValidationCode` using the genesis parameters.
1. Amend the `Parachains` list to reflect changes in registered parachains.
1. Amend the `Parathreads` set to reflect changes in registered parathreads.

## Initialization

1. Do pruning based on all entries in `PastCodePruning` with `BlockNumber <= now`. Update the corresponding `PastCodeMeta` and `PastCode` accordingly.

## Routines

* `schedule_para_initialize(ParaId, ParaGenesisArgs)`: schedule a para to be initialized at the next session.
* `schedule_para_cleanup(ParaId)`: schedule a para to be cleaned up at the next session.
* `schedule_code_upgrade(ParaId, ValidationCode, expected_at: BlockNumber)`: Schedule a future code upgrade of the given parachain, to be applied after inclusion of a block of the same parachain executed in the context of a relay-chain block with number >= `expected_at`.
* `note_new_head(ParaId, HeadData, BlockNumber)`: note that a para has progressed to a new head, where the new head was executed in the context of a relay-chain block with given number. This will apply pending code upgrades based on the block number provided.
* `validation_code_at(ParaId, at: BlockNumber, assume_intermediate: Option<BlockNumber>)`: Fetches the validation code to be used when validating a block in the context of the given relay-chain height. A second block number parameter may be used to tell the lookup to proceed as if an intermediate parablock has been included at the given relay-chain height. This may return past, current, or (with certain choices of `assume_intermediate`) future code. `assume_intermediate`, if provided, must be before `at`. If the validation code has been pruned, this will return `None`.
* `is_parathread(ParaId) -> bool`: Returns true if the para ID references any live parathread.

* `last_code_upgrade(id: ParaId, include_future: bool) -> Option<BlockNumber>`: The block number of the last scheduled upgrade of the requested para. Includes future upgrades if the flag is set. This is the `expected_at` number, not the `activated_at` number.
* `local_validation_data(id: ParaId) -> Option<LocalValidationData>`: Get the LocalValidationData of the given para, assuming the context is the parent block. Returns `None` if the para is not known.

## Finalization

No finalization routine runs for this module.
