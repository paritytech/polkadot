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

This is the full receipt type. The `ValidationData` are technically redundant with the `inner.relay_parent`, which uniquely describes the block in the blockchain from whose state these values are derived. The [`CandidateReceipt`](#candidate-receipt) variant is often used instead for this reason.

However, the Full Candidate Receipt type is useful as a means of avoiding the implicit dependency on availability of old blockchain state. In situations such as availability and approval, having the full description of the candidate within a self-contained struct is convenient.

```rust
/// All data pertaining to the execution of a para candidate.
struct FullCandidateReceipt {
	inner: CandidateReceipt,
	validation_data: ValidationData,
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
	/// The blake2-256 hash of the persisted validation data. These are extra parameters
	/// derived from relay-chain state that influence the validity of the block which
	/// must also be kept available for secondary checkers.
	validation_data_hash: Hash,
	/// The blake2-256 hash of the pov-block.
	pov_hash: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the pov_hash.
	signature: CollatorSignature,
}
```

## ValidationData

The validation data provide information about how to validate both the inputs and outputs of a candidate. There are two types of validation data: [persisted](#persistedvalidationdata) and [transient](#transientvalidationdata). Their respective sections of the guide elaborate on their functionality in more detail.

This information is derived from the chain state and will vary from para to para, although some of the fields may be the same for every para.

Persisted validation data are generally derived from some relay-chain state to form inputs to the validation function, and as such need to be persisted by the availability system to avoid dependence on availability of the relay-chain state. The backing phase of the inclusion pipeline ensures that everything that is included in a valid fork of the relay-chain already adheres to the transient constraints.

The validation data also serve the purpose of giving collators a means of ensuring that their produced candidate and the commitments submitted to the relay-chain alongside it will pass the checks done by the relay-chain when backing, and give validators the same understanding when determining whether to second or attest to a candidate.

Since the commitments of the validation function are checked by the relay-chain, secondary checkers can rely on the invariant that the relay-chain only includes para-blocks for which these checks have already been done. As such, there is no need for the validation data used to inform validators and collators about the checks the relay-chain will perform to be persisted by the availability system. Nevertheless, we expose it so the backing validators can validate the outputs of a candidate before voting to submit it to the relay-chain and so collators can collate candidates that satisfy the criteria implied these transient validation data.

Design-wise we should maintain two properties about this data structure:

1. The `ValidationData` should be relatively lightweight primarly because it is constructed during inclusion for each candidate.
1. To make contextual execution possible, `ValidationData` should be constructable only having access to the latest relay-chain state for the past `k` blocks. That implies
either that the relay-chain should maintain all the required data accessible or somehow provided indirectly with a header-chain proof and a state proof from there.

```rust
struct ValidationData {
    persisted: PersistedValidationData,
    transient: TransientValidationData,
}
```

## PersistedValidationData

Validation data that needs to be persisted for secondary checkers. See the section on [`ValidationData`](#validationdata) for more details.

```rust
struct PersistedValidationData {
	/// The parent head-data.
	parent_head: HeadData,
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
	///
	/// This informs a relay-chain backing check and the parachain logic.
	code_upgrade_allowed: Option<BlockNumber>,

	/// The relay-chain block number this is in the context of. This informs the collator.
	block_number: BlockNumber,
}
```

## TransientValidationData

These validation data are derived from some relay-chain state to check outputs of the validation function.

```rust
struct TransientValidationData {
	/// The maximum code size permitted, in bytes, of a produced validation code upgrade.
	///
	/// This informs a relay-chain backing check and the parachain logic.
	max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	///
	/// This informs a relay-chain backing check and the parachain collator.
	max_head_data_size: u32,
	/// The balance of the parachain at the moment of validation.
	balance: Balance,
	/// The list of MQC heads for the inbound channels paired with the sender para ids. This
	/// vector is sorted ascending by the para id and doesn't contain multiple entries with the same
	/// sender. This informs the collator.
	hrmp_mqc_heads: Vec<(ParaId, Hash)>,
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
	/// Messages directed to other paras routed via the relay chain.
	horizontal_messages: Vec<OutboundHrmpMessage>,
	/// Messages destined to be interpreted by the Relay chain itself.
	upward_messages: Vec<UpwardMessage>,
	/// The root of a block's erasure encoding Merkle tree.
	erasure_root: Hash,
	/// New validation code.
	new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	head_data: HeadData,
	/// The number of messages processed from the DMQ.
	processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are processed.
	hrmp_watermark: BlockNumber,
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
	/// The validation data, persisted and transient.
	validation_data: ValidationData,
	/// Messages directed to other paras routed via the relay chain.
	horizontal_messages: Vec<OutboundHrmpMessage>,
	/// Upwards messages to the relay chain.
	upwards_messages: Vec<UpwardsMessage>,
	/// Fees paid to the validators of the relay-chain.
	fees: Balance,
	/// The new validation code submitted by the execution, if any.
	new_validation_code: Option<ValidationCode>,
	/// The number of messages processed from the DMQ.
	processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are processed.
	hrmp_watermark: BlockNumber,
}
```
