# Candidate Types

Para candidates are some of the most common types, both within the runtime and on the Node-side.

In a way, this entire guide is about these candidates: how they are scheduled, constructed, backed, included, and challenged.

This section will describe the base candidate type, its components, and abridged counterpart.

## CandidateReceipt

This is the base receipt type. The `GlobalValidationSchedule` and the `LocalValidationData` are technically redundant with the `inner.relay_parent`, which uniquely describes the a block in the blockchain from whose state these values are derived. The [`AbridgedCandidateReceipt`](#abridgedcandidatereceipt) variant is often used instead for this reason.

However, the full CandidateReceipt type is useful as a means of avoiding the implicit dependency on availability of old blockchain state. In situations such as availability and approval, having the full description of the candidate within a self-contained struct is convenient.

```rust
/// All data pertaining to the execution of a para candidate.
struct CandidateReceipt {
	inner: AbridgedCandidateReceipt,
	/// The global validation schedule.
	global_validation: GlobalValidationSchedule,
	/// The local validation data.
	local_validation: LocalValidationData,
}
```

## AbridgedCandidateReceipt

Much info in a [`CandidateReceipt`](#candidatereceipt) is duplicated from the relay-chain state. When the corresponding relay-chain state is considered widely available, the Abridged Candidate Receipt should be favored.

Examples of situations where the state is readily available includes within the scope of work done by subsystems working on a given relay-parent, or within the logic of the runtime importing a backed candidate.

```rust
/// An abridged candidate-receipt.
struct AbridgedCandidateReceipt {
	/// The ID of the para this is a candidate for.
	para_id: Id,
	/// The hash of the relay-chain block this is executed in the context of.
	relay_parent: Hash,
	/// The head-data produced as a result of execution.
	head_data: HeadData,
	/// The collator's sr25519 public key.
	collator: CollatorId,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the head data, and the pov_hash.
	signature: CollatorSignature,
	/// The blake2-256 hash of the pov-block.
	pov_hash: Hash,
	/// Commitments made as a result of validation.
	commitments: CandidateCommitments,
}
```

## GlobalValidationSchedule

The global validation schedule comprises of information describing the global environment for para execution, as derived from a particular relay-parent. These are parameters that will apply to all parablocks executed in the context of this relay-parent.

> TODO: message queue watermarks (first upward messages, then XCMP channels)

```rust
/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate.
///
/// These are global parameters that apply to all candidates in a block.
struct GlobalValidationSchedule {
	/// The maximum code size permitted, in bytes.
	max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	max_head_data_size: u32,
	/// The relay-chain block number this is in the context of.
	block_number: BlockNumber,
}
```

## LocalValidationData

This is validation data needed for execution of candidate pertaining to a specific para and relay-chain block.

Unlike the [`GlobalValidationData`](#globalvalidationdata), which only depends on a relay-parent, this is parameterized both by a relay-parent and a choice of one of two options:
  1. Assume that the candidate pending availability on this para at the onset of the relay-parent is included.
  1. Assume that the candidate pending availability on this para at the onset of the relay-parent is timed-out.

This choice can also be expressed as a choice of which parent head of the para will be built on - either optimistically on the candidate pending availability or pessimistically on the one that is surely included.

Para validation happens optimistically before the block is authored, so it is not possible to predict with 100% accuracy what will happen in the earlier phase of the [`InclusionInherent`](/runtime/inclusioninherent.html) module where new availability bitfields and availability timeouts are processed. This is what will eventually define whether a candidate can be backed within a specific relay-chain block.

> TODO: determine if balance/fees are even needed here.

```rust
/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate. These fields are parachain-specific.
pub struct LocalValidationData {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The balance of the parachain at the moment of validation.
	pub balance: Balance,
	/// The blake2-256 hash of the validation code used to execute the candidate.
	pub validation_code_hash: Hash,
	/// Whether the parachain is allowed to upgrade its validation code.
	///
	/// This is `Some` if so, and contains the number of the minimum relay-chain
	/// height at which the upgrade will be applied, if an upgrade is signaled
	/// now.
	///
	/// A parachain should enact its side of the upgrade at the end of the first
	/// parablock executing in the context of a relay-chain block with at least this
	/// height. This may be equal to the current perceived relay-chain block height, in
	/// which case the code upgrade should be applied at the end of the signaling
	/// block.
	pub code_upgrade_allowed: Option<BlockNumber>,
}
```

## HeadData

Head data is a type-safe abstraction around bytes (`Vec<u8>`) for the purposes of representing heads of parachains or parathreads.

```rust
struct HeadData(Vec<u8>);
```

## CandidateCommitments

The execution and validation of parachain or parathread candidates produces a number of values which either must be committed to on the relay chain or committed to the state of the relay chain.

```rust
/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct CandidateCommitments {
	/// Fees paid from the chain to the relay chain validators.
	pub fees: Balance,
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
}
```

## Signing Context

This struct provides context to signatures by combining with various payloads to localize the signature to a particular session index and relay-chain hash. Having these fields included in the signature makes misbehavior attribution much simpler.

```rust
struct SigningContext {
	/// The relay-chain block hash this signature is in the context of.
	parent_hash: Hash,
	/// The session index this signature is in the context of.
	session_index: SessionIndex,
}
```
