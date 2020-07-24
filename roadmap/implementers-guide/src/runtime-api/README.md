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

More broadly, we have the goal of using Runtime APIs to write Node-side code that fulfills the requirements set by the Runtime. In particular, the constraints set forth by the [Scheduler](../runtime/scheduler.md) and [Inclusion](../runtime/inclusion.md) modules. These modules are responsible for advancing paras with a two-phase protocol where validators are first chosen to validate and back a candidate and then required to ensure availability of referenced data. In the second phase, validators are meant to attest to those para-candidates that they have their availability chunk for. As the Node-side code needs to generate the inputs into these two phases, the runtime API needs to transmit information from the runtime that is aware of the Availability Cores model instantiated by the Scheduler and Inclusion modules.

Node-side code is also responsible for detecting and reporting misbehavior performed by other validators, and the set of Runtime APIs needs to provide methods for observing live disputes and submitting reports as transactions.

The next sections will contain information on specific runtime APIs. The format is this:

```rust
/// Fetch the value of the runtime API at the block.
///
/// Definitionally, the `at` parameter cannot be any block that is not in the chain.
/// Thus the return value is unconditional. However, for in-practice implementations
/// it may be possible to provide an `at` parameter as a hash, which may not refer to a
/// valid block or one which implements the runtime API. In those cases it would be
/// best for the implementation to return an error indicating the failure mode.
fn some_runtime_api(at: Block, arg1: Type1, arg2: Type2, ...) -> ReturnValue;
```

## Validators

Yields the validator-set at the state of a given block. This validator set is always the one responsible for backing parachains in the child of the provided block.

```rust
fn validators(at: Block) -> Vec<ValidatorId>;
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
	/// Returns the index of the group needed to validate the core at the given index,
	/// assuming the given amount of cores/groups.
	fn group_for_core(&self, core_index, cores) -> GroupIndex;

	/// Returns the block number of the next rotation after the current block. If the current block
	/// is 10 and the rotation frequency is 5, this should return 15.
	///
	/// If the group rotation frequency is 0, returns 0.
	fn next_rotation_at(&self) -> BlockNumber;

	/// Returns the block number of the last rotation before or including the current block. If the
	/// current block is 10 and the rotation frequency is 5, this should return 10.
	///
	/// If the group rotation frequency is 0, returns 0.
	fn last_rotation_at(&self) -> BlockNumber;
}

/// Returns the validator groups and rotation info localized based on the block whose state
/// this is invoked on. Note that `now` in the `GroupRotationInfo` should be the successor of
/// the number of the block.
fn validator_groups(at: Block) -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo);
```

## Availability Cores

Yields information on all availability cores. Cores are either free or occupied. Free cores can have paras assigned to them. Occupied cores don't, but they can become available part-way through a block due to bitfields and then have something scheduled on them. To allow optimistic validation of candidates, the occupied cores are accompanied by information on what is upcoming. This information can be leveraged when validators perceive that there is a high likelihood of a core becoming available based on bitfields seen, and then optimistically validate something that would become scheduled based on that, although there is no guarantee on what the block producer will actually include in the block.

```rust
fn availability_cores(at: Block) -> Vec<CoreState>;
```

This is all the information that a validator needs about scheduling for the current block. It includes all information on [Scheduler](../runtime/scheduler.md) core-assignments and [Inclusion](../runtime/inclusion.md) state of blocks occupying availability cores. It includes data necessary to determine not only which paras are assigned now, but which cores are likely to become freed after processing bitfields, and exactly which bitfields would be necessary to make them so.

```rust
struct OccupiedCore {
	/// The ID of the para occupying the core.
	para_id: ParaId,
	/// If this core is freed by availability, this is the assignment that is next up on this
	/// core, if any. None if there is nothing queued for this core.
	next_up_on_available: Option<ScheduledCore>,
	/// The relay-chain block number this began occupying the core at.
	occupied_since: BlockNumber,
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
	/// The group assigned to distribute availability pieces of this candidate.
	group_responsible: GroupIndex,
}

struct ScheduledCore {
	/// The ID of a para scheduled.
	para_id: ParaId,
	/// The collator required to author the block, if any.
	collator: Option<CollatorId>,
}

enum CoreState {
	/// The core is currently occupied.
	Occupied(OccupiedCore),
	/// The core is currently free, with a para scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	Scheduled(ScheduledCore),
	/// The core is currently free and there is nothing scheduled. This can be the case for parathread
	/// cores when there are no parathread blocks queued. Parachain cores will never be left idle.
	Free,
}
```

## Global Validation Schedule

Yields the [`GlobalValidationData`](../types/candidate.md#globalvalidationschedule) at the state of a given block. This applies to all para candidates with the relay-parent equal to that block.

```rust
fn global_validation_data(at: Block) -> GlobalValidationData;
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
	Free,
}

/// Returns the local validation data for the given para and occupied core assumption.
///
/// Returns `None` if either the para is not registered or the assumption is `Freed`
/// and the para already occupies a core.
fn local_validation_data(at: Block, ParaId, OccupiedCoreAssumption) -> Option<LocalValidationData>;
```

## Session Index

Get the session index that is expected at the child of a block.

In the [`Initializer`](../runtime/initializer.md) module, session changes are buffered by one block. The session index of the child of any relay block is always predictable by that block's state.

This session index can be used to derive a [`SigningContext`](../types/candidate.md#signing-context).

```rust
/// Returns the session index expected at a child of the block.
fn session_index_for_child(at: Block) -> SessionIndex;
```

## Validation Code

Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.

```rust
fn validation_code(at: Block, ParaId, OccupiedCoreAssumption) -> Option<ValidationCode>;
```

## Candidate Pending Availability

Get the receipt of a candidate pending availability. This returns `Some` for any paras assigned to occupied cores in `availability_cores` and `None` otherwise.

```rust
fn candidate_pending_availability(at: Block, ParaId) -> Option<CommittedCandidateReceipt>;
```

## Candidate Events

Yields a vector of events concerning candidates that occurred within the given block.

```rust
enum CandidateEvent {
	/// This candidate receipt was backed in the most recent block.
	CandidateBacked(CandidateReceipt, HeadData),
	/// This candidate receipt was included and became a parablock at the most recent block.
	CandidateIncluded(CandidateReceipt, HeadData),
	/// This candidate receipt was not made available in time and timed out.
	CandidateTimedOut(CandidateReceipt, HeadData),
}

fn candidate_events(at: Block) -> Vec<CandidateEvent>;
```
