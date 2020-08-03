# Candidate Types

Para candidates are some of the most common types, both within the runtime and on the Node-side.
Candidates are the fundamental datatype for advancing parachains and parathreads, encapsulating the collator's signature, the context of the parablock, the commitments to the output, and a commitment to the data which proves it valid.

In a way, this entire guide is about these candidates: how they are scheduled, constructed, backed, included, and challenged.

This section will describe the base candidate type, its components, and variants that contain extra data.

## Para Id

A unique 32-bit identifier referring to a specific para (chain or thread). The relay-chain runtime guarantees that `ParaId`s are unique for the duration of any session, but recycling and reuse over a longer period of time is permitted.

```rust
struct ParaId(u32);
```

## Candidate Receipt

Much info in a [`FullCandidateReceipt`](#full-candidate-receipt) is duplicated from the relay-chain state. When the corresponding relay-chain state is considered widely available, the Candidate Receipt should be favored over the `FullCandidateReceipt`.

Examples of situations where the state is readily available includes within the scope of work done by subsystems working on a given relay-parent, or within the logic of the runtime importing a backed candidate.

```rust
/// A candidate-receipt.
struct CandidateReceipt {
	/// The descriptor of the candidate.
	descriptor: CandidateDescriptor,
	/// The hash of the encoded commitments made as a result of candidate execution.
	commitments_hash: Hash,
}
```

## Full Candidate Receipt

This is the full receipt type. The `GlobalValidationData` and the `LocalValidationData` are technically redundant with the `inner.relay_parent`, which uniquely describes the a block in the blockchain from whose state these values are derived. The [`CandidateReceipt`](#candidate-receipt) variant is often used instead for this reason.

However, the Full Candidate Receipt type is useful as a means of avoiding the implicit dependency on availability of old blockchain state. In situations such as availability and approval, having the full description of the candidate within a self-contained struct is convenient.

```rust
/// All data pertaining to the execution of a para candidate.
struct FullCandidateReceipt {
	inner: CandidateReceipt,
	/// The global validation schedule.
	global_validation: GlobalValidationData,
	/// The local validation data.
	local_validation: LocalValidationData,
}
```

## Committed Candidate Receipt

This is a variant of the candidate receipt which includes the commitments of the candidate receipt alongside the descriptor. This should be favored over the [`Candidate Receipt`](#candidate-receipt) in situations where the candidate is not going to be executed but the actual data committed to is important. This is often the case in the backing phase.

The hash of the committed candidate receipt will be the same as the corresponding [`Candidate Receipt`](#candidate-receipt), because it is computed by first hashing the encoding of the commitments to form a plain [`Candidate Receipt`](#candidate-receipt).

```rust
/// A candidate-receipt with commitments directly included.
struct CommittedCandidateReceipt {
	/// The descriptor of the candidate.
	descriptor: CandidateDescriptor,
	/// The commitments of the candidate receipt.
	commitments: CandidateCommitments,
}
```

## Candidate Descriptor

This struct is pure description of the candidate, in a lightweight format.

```rust
/// A unique descriptor of the candidate receipt.
struct CandidateDescriptor {
	/// The ID of the para this is a candidate for.
	para_id: ParaId,
	/// The hash of the relay-chain block this is executed in the context of.
	relay_parent: Hash,
	/// The collator's sr25519 public key.
	collator: CollatorId,
	/// The blake2-256 hash of the local and global validation data. These are extra parameters
	/// derived from relay-chain state that influence the validity of the block.
	validation_data_hash: Hash,
	/// The blake2-256 hash of the pov-block.
	pov_hash: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the pov_hash.
	signature: CollatorSignature,
}
```


## GlobalValidationData

The global validation schedule comprises of information describing the global environment for para execution, as derived from a particular relay-parent. These are parameters that will apply to all parablocks executed in the context of this relay-parent.

```rust
/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate.
///
/// These are global parameters that apply to all candidates in a block.
struct GlobalValidationData {
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

Para validation happens optimistically before the block is authored, so it is not possible to predict with 100% accuracy what will happen in the earlier phase of the [`InclusionInherent`](../runtime/inclusioninherent.md) module where new availability bitfields and availability timeouts are processed. This is what will eventually define whether a candidate can be backed within a specific relay-chain block.

Design-wise we should maintain two properties about this data structure:

1. The `LocalValidationData` should be relatively lightweight primarly because it is constructed during inclusion for each candidate.
1. To make contextual execution possible, `LocalValidationData` should be constructable only having access to the latest relay-chain state for the past `k` blocks. That implies
either that the relay-chain should maintain all the required data accessible or somehow provided indirectly with a header-chain proof and a state proof from there.

> TODO: determine if balance/fees are even needed here.
> TODO: message queue watermarks (first downward messages, then XCMP channels)

```rust
/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate. These fields are parachain-specific.
struct LocalValidationData {
	/// The parent head-data.
	parent_head: HeadData,
	/// The balance of the parachain at the moment of validation.
	balance: Balance,
	/// The blake2-256 hash of the validation code used to execute the candidate.
	validation_code_hash: Hash,
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
	code_upgrade_allowed: Option<BlockNumber>,
}
```

## HeadData

Head data is a type-safe abstraction around bytes (`Vec<u8>`) for the purposes of representing heads of parachains or parathreads.

```rust
struct HeadData(Vec<u8>);
```

## Candidate Commitments

The execution and validation of parachain or parathread candidates produces a number of values which either must be committed to on the relay chain or committed to the state of the relay chain.

```rust
/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
struct CandidateCommitments {
	/// Fees paid from the chain to the relay chain validators.
	fees: Balance,
	/// Messages destined to be interpreted by the Relay chain itself.
	upward_messages: Vec<UpwardMessage>,
	/// The root of a block's erasure encoding Merkle tree.
	erasure_root: Hash,
	/// New validation code.
	new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	head_data: HeadData,
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

## Validation Outputs

This struct encapsulates the outputs of candidate validation.

```rust
struct ValidationOutputs {
	/// The head-data produced by validation.
	head_data: HeadData,
	/// The global validation schedule.
	global_validation_data: GlobalValidationData,
	/// The local validation data.
	local_validation_data: LocalValidationData,
	/// Upwards messages to the relay chain.
	upwards_messages: Vec<UpwardsMessage>,
	/// Fees paid to the validators of the relay-chain.
	fees: Balance,
	/// The new validation code submitted by the execution, if any.
	new_validation_code: Option<ValidationCode>,
}
```
