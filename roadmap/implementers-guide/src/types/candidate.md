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
	persisted_validation_data_hash: Hash,
	/// The blake2-256 hash of the pov-block.
	pov_hash: Hash,
	/// The root of a block's erasure encoding Merkle tree.
	erasure_root: Hash,
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

Furthermore, the validation data acts as a way to authorize the additional data the collator needs to pass to the validation
function. For example, the validation function can check whether the incoming messages (e.g. downward messages) were actually
sent by using the data provided in the validation data using so called MQC heads.

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
	/// The relay-chain block number this is in the context of. This informs the collator.
	block_number: BlockNumber,
	/// The relay-chain block storage root this is in the context of.
	relay_storage_root: Hash,
	/// The MQC head for the DMQ.
	///
	/// The DMQ MQC head will be used by the validation function to authorize the downward messages
	/// passed by the collator.
	dmq_mqc_head: Hash,
	/// The list of MQC heads for the inbound channels paired with the sender para ids. This
	/// vector is sorted ascending by the para id and doesn't contain multiple entries with the same
	/// sender.
	///
	/// The HRMP MQC heads will be used by the validation function to authorize the input messages passed
	/// by the collator.
	hrmp_mqc_heads: Vec<(ParaId, Hash)>,
}
```

## TransientValidationData

These validation data are derived from some relay-chain state to check outputs of the validation function.

It's worth noting that all the data is collected **before** the candidate execution.

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
	/// A copy of `config.max_upward_message_num_per_candidate` for checking that a candidate doesn't
	/// send more messages than permitted.
	config_max_upward_message_num_per_candidate: u32,
	/// The number of messages pending of the downward message queue.
	dmq_length: u32,
	/// A part of transient validation data related to HRMP.
	hrmp: HrmpTransientValidationData,
}

struct HrmpTransientValidationData {
	/// A vector that enumerates the list of blocks in which there was at least one HRMP message
	/// received.
	///
	/// The first number in the vector, if any, is always greater than the HRMP watermark. The
	/// elements are ordered by ascending the block number. The vector doesn't contain duplicates.
	digest: Vec<BlockNumber>,
	/// The watermark of the HRMP. That is, the block number up to which (inclusive) all HRMP messages
	/// sent to the parachain are processed.
	watermark: BlockNumber,
	/// A mapping that specifies if the parachain can send an HRMP message to the given recipient
	/// channel. A candidate can send a message only to the recipients that are present in this
	/// mapping. The number elements in this vector corresponds to the number of egress channels.
	/// Since it's a mapping there can't be two items with same `ParaId`.
	egress_limits: Vec<(ParaId, HrmpChannelLimits)>,
	/// A vector of paras that have a channel to this para. The number of elements in this vector
	/// correponds to the number of ingress channels. The items are ordered ascending by `ParaId`.
	/// The vector doesn't contain two entries with the same `ParaId`.
	ingress_senders: Vec<ParaId>,
	/// A vector of open requests in which the para participates either as sender or recipient. The
	/// items are ordered ascending by `HrmpChannelId`. The vector doesn't contain two entries
	/// with the same `HrmpChannelId`.
	open_requests: Vec<(HrmpChannelId, HrmpAbridgedOpenChannelRequest)>,
	/// A vector of close requests in which the para participates either as sender or recipient.
	/// The vector doesn't contain two entries with the same `HrmpChannelId`.
	close_requests: Vec<HrmpChannelId>,
	/// The maximum number of inbound channels the para is allowed to have. This is a copy of either
	/// `config.hrmp_max_parachain_inbound_channels` or `config.hrmp_max_parathread_inbound_channels`
	/// depending on the type of this para.
	config_max_inbound_channels: u32,
	/// The maximum number of outbound channels the para is allowed to have. This is a copy of either
	/// `config.hrmp_max_parachain_outbound_channels` or `config.hrmp_max_parathread_outbound_channels`
	/// depending on the type of this para.
	config_max_outbound_channels: u32,
}

/// A shorter version of `HrmpOpenChannelRequest`.
struct HrmpAbridgedOpenChannelRequest {
	confirmed: bool,
}

struct HrmpChannelLimits {
	/// Indicates if the channel is already full and cannot accept any more messages.
	is_full: bool,
	/// A message sent to the channel can occupy only that many bytes.
	available_size: u32,
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
	/// Messages directed to other paras routed via the relay chain.
	horizontal_messages: Vec<OutboundHrmpMessage>,
	/// Messages destined to be interpreted by the Relay chain itself.
	upward_messages: Vec<UpwardMessage>,
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