# Runtime APIs

Runtime APIs are the means by which the node-side code extracts information from the state of the runtime.

Every block in the relay-chain contains a *state root* which is the root hash of a state trie encapsulating all storage of runtime modules after execution of the block. This is a cryptographic commitment to a unique state. We use the terminology of accessing the *state at* a block to refer accessing the state referred to by the state root of that block.

Although Runtime APIs are often used for simple storage access, they are actually empowered to do arbitrary computation. The implementation of the Runtime APIs lives within the Runtime as Wasm code and exposes extern functions that can be invoked with arguments and have a return value. Runtime APIs have access to a variety of host functions, which are contextual functions provided by the Wasm execution context, that allow it to carry out many different types of behaviors.

Abilities provided by host functions includes:
  * State Access
  * Offchain-DB Access
  * Submitting transactions to the transaction queue
  * Optimized versions of cryptographic functions
  * More

So it is clear that Runtime APIs are a versatile and powerful tool to leverage the state of the chain. In general, we will use Runtime APIs for these purposes:
  * Access of a storage item
  * Access of a bundle of related storage items
  * Deriving a value from storage based on arguments
  * Submitting misbehavior reports

The next sections will contain information on specific runtime APIs.

## Validators

Yields the validator-set at the state of a given block. This validator set is always the one responsible for backing parachains in the child of the provided block.

```rust
fn validators() -> Vec<ValidatorId>;
```

## Validator Groups

Yields the validator groups used during the current session. The validators in the groups are referred to by their index into the validator-set.

```rust
/// A helper data-type for tracking validator-group rotations.
struct GroupRotationInfo {
	session_start_block: BlockNumber,
	group_rotation_frequency: BlockNumber,
	now: BlockNumber,
}

impl GroupRotationInfo {
	/// Returns the index of the group needed to validate the core at the given index.
	fn group_for_core(usize) -> usize;
}

/// Returns the validator groups and rotation info localized based on the block whose state
/// this is invoked on. Note that `now` in the `GroupRotationInfo` should be the successor of
/// the number of the block.
fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo);
```

## ScheduledParas

Yields information on the scheduling of paras.

```rust
fn scheduled_paras() -> ScheduledParas;
```

This is all the information that a validator needs about scheduling for the current block. It includes all information on [Scheduler](../runtime/scheduler.md) core-assignments and [Inclusion](../runtime/inclusion.md) state of blocks occupying availability cores. It includes data necessary to determine not only which paras are assigned now, but which cores are likely to become freed after processing bitfields, and exactly which bitfields would be necessary to make them so.

```rust
struct OccupiedCore {
	/// The ID of the para occupying the core.
	para: ParaId,
	/// If this core is freed by availability, this is the assignment that is next up on this
	/// core, if any. None if there is nothing queued for this core.
	next_up_on_available: Option<ScheduledCore>,
	/// The relay-chain block this will time-out at, if any.
	time_out_at: BlockNumber,
	/// If this core is freed by being timed-out, this is the assignment that is next up on this
	/// core. None if there is nothing queued for this core or there is no possibility of timing
	/// out.
	next_up_on_time_out: Option<ScheduledCore>,
	/// A bitfield with 1 bit for each validator in the set. `1` bits mean that the corresponding
	/// validators has attested to availability on-chain. A 2/3+ majority of `1` bits means that
	/// this will be available.
	availability: Bitfield,
}

struct ScheduledCore {
	/// The ID of a para scheduled.
	para: ParaId,
	/// The collator required to author the block, if any.
	collator: Option<CollatorId>,
}

enum CoreState {
	/// The core is currently occupied by the given `ParaId`.
	Occupied(OccupiedCore),
	/// The core is currently free, with the given `ParaId` scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	Scheduled(ScheduledCore)
}
```

```rust
struct ScheduledParas {
	/// All cores, along with their state. There should be as many items in this vector as there
	/// are cores in the system.
	cores: Vec<CoreState>,
}
```

## Global Validation Schedule

Yields the [`GlobalValidationSchedule`](../types/candidate.md#globalvalidationschedule) at the state of a given block. This applies to all para candidates with the relay-parent equal to that block.

```rust
fn global_validation_schedule() -> GlobalValidationSchedule;
```

## Local Validation Data

Yields the [`LocalValidationData`](../types/candidate.md#localvalidationdata) for the given [`ParaId`](../types/candidate.md#paraid) along with an assumption that should be used if the para currently occupies a core: whether the candidate occupying that core should be assumed to have been made available and included or timed out and discarded, along with a third option to assert that the core was not occupied. This choice affects everything from the parent head-data, the validation code, and the state of message-queues. Typically, users will take the assumption that either the core was free or that the occupying candidate was included, as timeouts are expected only in adversarial circumstances and even so, only in a small minority of blocks directly following validator set rotations.

The documentation of [`LocalValidationData`](../types/candidate.md#localvalidationdata) has more information on this dichotomy.

```rust
/// An assumption being made about the state of an occupied core.
enum OccupiedCoreAssumption {
	/// The candidate occupying the core was made available and included to free the core.
	Included,
	/// The candidate occupying the core timed out and freed the core without advancing the para.
	TimedOut,
	/// The core was not occupied to begin with.
	Freed,
}

/// Returns the local validation data for the given para and occupied core assumption.
///
/// Returns `None` if either the para is not registered or the assumption is `Freed`
/// and the para already occupies a core.
fn local_validation_data(ParaId, OccupiedCoreAssumption) -> Option<LocalValidationData>;
```
