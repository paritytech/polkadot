# Overseer Protocol

This chapter contains message types sent to and from the overseer, and the underlying subsystem message types that are transmitted using these.

## Overseer Signal

Signals from the overseer to a subsystem to request change in execution that has to be obeyed by the subsystem.

```rust
enum OverseerSignal {
  /// Signal about a change in active leaves.
  ActiveLeavesUpdate(ActiveLeavesUpdate),
  /// Conclude all operation.
  Conclude,
}
```

All subsystems have their own message types; all of them need to be able to listen for overseer signals as well. There are currently two proposals for how to handle that with unified communication channels:

1. Retaining the `OverseerSignal` definition above, add `enum FromOverseer<T> {Signal(OverseerSignal), Message(T)}`.
1. Add a generic varint to `OverseerSignal`: `Message(T)`.

Either way, there will be some top-level type encapsulating messages from the overseer to each subsystem.

## Active Leaves Update

Indicates a change in active leaves. Activated leaves should have jobs, whereas deactivated leaves should lead to winding-down of work based on those leaves.

```rust
struct ActiveLeavesUpdate {
	activated: [Hash], // in practice, these should probably be a SmallVec
	deactivated: [Hash],
}
```

## All Messages

> TODO (now)

## Availability Distribution Message

Messages received by the availability distribution subsystem.

This is a network protocol that receives messages of type [`AvailabilityDistributionV1Message`][AvailabilityDistributionV1NetworkMessage].

```rust
enum AvailabilityDistributionMessage {
	/// Distribute an availability chunk to other validators.
	DistributeChunk(Hash, ErasureChunk),
	/// Fetch an erasure chunk from network by candidate hash and chunk index.
	FetchChunk(Hash, u32),
	/// Event from the network.
	/// An update on network state from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<AvailabilityDistributionV1Message>),
}
```

## Availability Store Message

Messages to and from the availability store.

```rust
enum AvailabilityStoreMessage {
	/// Query the `AvailableData` of a candidate by hash.
	QueryAvailableData(Hash, ResponseChannel<Option<AvailableData>>),
	/// Query whether an `AvailableData` exists within the AV Store.
	QueryDataAvailability(Hash, ResponseChannel<bool>),
	/// Query a specific availability chunk of the candidate's erasure-coding by validator index.
	/// Returns the chunk and its inclusion proof against the candidate's erasure-root.
	QueryChunk(Hash, ValidatorIndex, ResponseChannel<Option<AvailabilityChunkAndProof>>),
	/// Store a specific chunk of the candidate's erasure-coding by validator index, with an
	/// accompanying proof.
	StoreChunk(Hash, ValidatorIndex, AvailabilityChunkAndProof, ResponseChannel<Result<()>>),
	/// Store `AvailableData`. If `ValidatorIndex` is provided, also store this validator's
	/// `AvailabilityChunkAndProof`.
	StoreAvailableData(Hash, Option<ValidatorIndex>, u32, AvailableData, ResponseChannel<Result<()>>),
}
```

## Bitfield Distribution Message

Messages received by the bitfield distribution subsystem.
This is a network protocol that receives messages of type [`BitfieldDistributionV1Message`][BitfieldDistributionV1NetworkMessage].

```rust
enum BitfieldDistributionMessage {
	/// Distribute a bitfield signed by a validator to other validators.
	/// The bitfield distribution subsystem will assume this is indeed correctly signed.
	DistributeBitfield(relay_parent, SignedAvailabilityBitfield),
	/// Receive a network bridge update.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<BitfieldDistributionV1Message>),
}
```

## Bitfield Signing Message

Currently, the bitfield signing subsystem receives no specific messages.

```rust
/// Non-instantiable message type
enum BitfieldSigningMessage { }
```

## Candidate Backing Message

```rust
enum CandidateBackingMessage {
  /// Requests a set of backable candidates that could be backed in a child of the given
  /// relay-parent, referenced by its hash.
  GetBackedCandidates(Hash, ResponseChannel<Vec<NewBackedCandidate>>),
  /// Note that the Candidate Backing subsystem should second the given candidate in the context of the
  /// given relay-parent (ref. by hash). This candidate must be validated using the provided PoV.
  /// The PoV is expected to match the `pov_hash` in the descriptor.
  Second(Hash, CandidateReceipt, PoV),
  /// Note a peer validator's statement about a particular candidate. Disagreements about validity must be escalated
  /// to a broader check by Misbehavior Arbitration. Agreements are simply tallied until a quorum is reached.
  Statement(Statement),
}
```

## Candidate Selection Message

These messages are sent to the [Candidate Selection subsystem](../node/backing/candidate-selection.md) as a means of providing feedback on its outputs.

```rust
enum CandidateSelectionMessage {
  /// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
  Invalid(CandidateReceipt),
}
```

## Chain API Message

The Chain API subsystem is responsible for providing an interface to chain data.

```rust
enum ChainApiMessage {
	/// Get the block number by hash.
	/// Returns `None` if a block with the given hash is not present in the db.
	BlockNumber(Hash, ResponseChannel<Result<Option<BlockNumber>, Error>>),
	/// Get the finalized block hash by number.
	/// Returns `None` if a block with the given number is not present in the db.
	/// Note: the caller must ensure the block is finalized.
	FinalizedBlockHash(BlockNumber, ResponseChannel<Result<Option<Hash>, Error>>),
	/// Get the last finalized block number.
	/// This request always succeeds.
	FinalizedBlockNumber(ResponseChannel<Result<BlockNumber, Error>>),
	/// Request the `k` ancestors block hashes of a block with the given hash.
	/// The response channel may return a `Vec` of size up to `k`
	/// filled with ancestors hashes with the following order:
	/// `parent`, `grandparent`, ...
	Ancestors {
		/// The hash of the block in question.
		hash: Hash,
		/// The number of ancestors to request.
		k: usize,
		/// The response channel.
		response_channel: ResponseChannel<Result<Vec<Hash>, Error>>,
	}
}
```

## Collator Protocol Message

Messages received by the [Collator Protocol subsystem](../node/collators/collator-protocol.md)

This is a network protocol that receives messages of type [`CollatorProtocolV1Message`][CollatorProtocolV1NetworkMessage].

```rust
enum CollatorProtocolMessage {
	/// Signal to the collator protocol that it should connect to validators with the expectation
	/// of collating on the given para. This is only expected to be called once, early on, if at all,
	/// and only by the Collation Generation subsystem. As such, it will overwrite the value of
	/// the previous signal.
	///
	/// This should be sent before any `DistributeCollation` message.
	CollateOn(ParaId),
	/// Provide a collation to distribute to validators.
	DistributeCollation(CandidateReceipt, PoV),
	/// Fetch a collation under the given relay-parent for the given ParaId.
	FetchCollation(Hash, ParaId, ResponseChannel<(CandidateReceipt, PoV)>),
	/// Report a collator as having provided an invalid collation. This should lead to disconnect
	/// and blacklist of the collator.
	ReportCollator(CollatorId),
	/// Note a collator as having provided a good collation.
	NoteGoodCollation(CollatorId),
}
```

## Network Bridge Message

Messages received by the network bridge. This subsystem is invoked by others to manipulate access
to the low-level networking code.

```rust
/// Peer-sets handled by the network bridge.
enum PeerSet {
	/// The collation peer-set is used to distribute collations from collators to validators.
	Collation,
	/// The validation peer-set is used to distribute information relevant to parachain
	/// validation among validators. This may include nodes which are not validators,
	/// as some protocols on this peer-set are expected to be gossip.
	Validation,
}

enum NetworkBridgeMessage {
	/// Report a cost or benefit of a peer. Negative values are costs, positive are benefits.
	ReportPeer(PeerSet, PeerId, cost_benefit: i32),
	/// Send a message to one or more peers on the validation peerset.
	SendValidationMessage([PeerId], ValidationProtocolV1),
	/// Send a message to one or more peers on the collation peerset.
	SendCollationMessage([PeerId], ValidationProtocolV1),
	/// Connect to peers who represent the given `ValidatorId`s at the given relay-parent.
	///
	/// Also accepts a response channel by which the issuer can learn the `PeerId`s of those
	/// validators.
	ConnectToValidators(PeerSet, [ValidatorId], ResponseChannel<[(ValidatorId, PeerId)]>>),
}
```

## Misbehavior Report

```rust
enum MisbehaviorReport {
  /// These validator nodes disagree on this candidate's validity, please figure it out
  ///
  /// Most likely, the list of statments all agree except for the final one. That's not
  /// guaranteed, though; if somehow we become aware of lots of
  /// statements disagreeing about the validity of a candidate before taking action,
  /// this message should be dispatched with all of them, in arbitrary order.
  ///
  /// This variant is also used when our own validity checks disagree with others'.
  CandidateValidityDisagreement(CandidateReceipt, Vec<SignedFullStatement>),
  /// I've noticed a peer contradicting itself about a particular candidate
  SelfContradiction(CandidateReceipt, SignedFullStatement, SignedFullStatement),
  /// This peer has seconded more than one parachain candidate for this relay parent head
  DoubleVote(CandidateReceipt, SignedFullStatement, SignedFullStatement),
}
```

If this subsystem chooses to second a parachain block, it dispatches a `CandidateBackingSubsystemMessage`.

## PoV Distribution Message

This is a network protocol that receives messages of type [`PoVDistributionV1Message`][PoVDistributionV1NetworkMessage].

```rust
enum PoVDistributionMessage {
	/// Fetch a PoV from the network.
	///
	/// This `CandidateDescriptor` should correspond to a candidate seconded under the provided
	/// relay-parent hash.
	FetchPoV(Hash, CandidateDescriptor, ResponseChannel<PoV>),
	/// Distribute a PoV for the given relay-parent and CandidateDescriptor.
	/// The PoV should correctly hash to the PoV hash mentioned in the CandidateDescriptor
	DistributePoV(Hash, CandidateDescriptor, PoV),
	/// An update from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<PoVDistributionV1Message>),
}
```

## Provisioner Message

```rust
/// This data becomes intrinsics or extrinsics which should be included in a future relay chain block.
enum ProvisionableData {
  /// This bitfield indicates the availability of various candidate blocks.
  Bitfield(Hash, SignedAvailabilityBitfield),
  /// The Candidate Backing subsystem believes that this candidate is valid, pending availability.
  BackedCandidate(BackedCandidate),
  /// Misbehavior reports are self-contained proofs of validator misbehavior.
  MisbehaviorReport(Hash, MisbehaviorReport),
  /// Disputes trigger a broad dispute resolution process.
  Dispute(Hash, Signature),
}

/// This data needs to make its way from the provisioner into the InherentData.
///
/// There, it is used to construct the InclusionInherent.
type ProvisionerInherentData = (SignedAvailabilityBitfields, Vec<BackedCandidate>);

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
enum ProvisionerMessage {
  /// This message allows potential block authors to be kept updated with all new authorship data
  /// as it becomes available.
  RequestBlockAuthorshipData(Hash, Sender<ProvisionableData>),
  /// This message allows external subsystems to request the set of bitfields and backed candidates
  /// associated with a particular potential block hash.
  ///
  /// This is expected to be used by a proposer, to inject that information into the InherentData
  /// where it can be assembled into the InclusionInherent.
  RequestInherentData(Hash, oneshot::Sender<ProvisionerInherentData>),
  /// This data should become part of a relay chain block
  ProvisionableData(ProvisionableData),
}
```

## Runtime API Message

The Runtime API subsystem is responsible for providing an interface to the state of the chain's runtime.

This is fueled by an auxiliary type encapsulating all request types defined in the Runtime API section of the guide.

> TODO: link to the Runtime API section. Not possible currently because of https://github.com/Michael-F-Bryan/mdbook-linkcheck/issues/25. Once v0.7.1 is released it will work.

```rust
enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(ResponseChannel<Vec<ValidatorId>>),
	/// Get the validator groups and rotation info.
	ValidatorGroups(ResponseChannel<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>),
	/// Get the session index for children of the block. This can be used to construct a signing
	/// context.
	SessionIndex(ResponseChannel<SessionIndex>),
	/// Get the validation code for a specific para, using the given occupied core assumption.
	ValidationCode(ParaId, OccupiedCoreAssumption, ResponseChannel<Option<ValidationCode>>),
	/// Get the persisted validation data at the state of a given block for a specific para,
	/// with the given occupied core assumption.
	PersistedValidationData(
		ParaId,
		OccupiedCoreAssumption,
		ResponseChannel<Option<PersistedValidationData>>,
	),
	/// Get the full validation data for a specific para, with the given occupied core assumption.
	FullValidationData(
		ParaId,
		OccupiedCoreAssumption,
		ResponseChannel<Option<ValidationData>>,
	),
	/// Get information about all availability cores.
	AvailabilityCores(ResponseChannel<Vec<CoreState>>),
	/// Get a committed candidate receipt for all candidates pending availability.
	CandidatePendingAvailability(ParaId, ResponseChannel<Option<CommittedCandidateReceipt>>),
	/// Get all events concerning candidates in the last block.
	CandidateEvents(ResponseChannel<Vec<CandidateEvent>>),
}

enum RuntimeApiMessage {
	/// Make a request of the runtime API against the post-state of the given relay-parent.
	Request(Hash, RuntimeApiRequest),
}
```

## Statement Distribution Message

The Statement Distribution subsystem distributes signed statements and candidates from validators to other validators. It does this by distributing full statements, which embed the candidate receipt, as opposed to compact statements which don't.
It receives updates from the network bridge and signed statements to share with other validators.

This is a network protocol that receives messages of type [`StatementDistributionV1Message`][StatementDistributionV1NetworkMessage].

```rust
enum StatementDistributionMessage {
	/// An update from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<StatementDistributionV1Message>),
	/// We have validated a candidate and want to share our judgment with our peers.
	/// The hash is the relay parent.
	///
	/// The statement distribution subsystem assumes that the statement should be correctly
	/// signed.
	Share(Hash, SignedFullStatement),
}
```

## Validation Request Type

Various modules request that the [Candidate Validation subsystem](../node/utility/candidate-validation.md) validate a block with this message. It returns [`ValidationOutputs`](candidate.md#validationoutputs) for successful validation.

```rust

/// Result of the validation of the candidate.
enum ValidationResult {
	/// Candidate is valid, and here are the outputs. In practice, this should be a shared type
	/// so that validation caching can be done.
	Valid(ValidationOutputs),
	/// Candidate is invalid.
	Invalid,
}

/// Messages issued to the candidate validation subsystem.
///
/// ## Validation Requests
///
/// Validation requests made to the subsystem should return an error only on internal error.
/// Otherwise, they should return either `Ok(ValidationResult::Valid(_))` or `Ok(ValidationResult::Invalid)`.
enum CandidateValidationMessage {
	/// Validate a candidate with provided parameters. This will implicitly attempt to gather the
	/// `OmittedValidationData` and `ValidationCode` from the runtime API of the chain,
	/// based on the `relay_parent` of the `CandidateDescriptor`.
	///
	/// If there is no state available which can provide this data or the core for
	/// the para is not free at the relay-parent, an error is returned.
	ValidateFromChainState(CandidateDescriptor, PoV, ResponseChannel<Result<ValidationResult>>),

	/// Validate a candidate with provided parameters. Explicitly provide the `PersistedValidationData`
	/// and `ValidationCode` so this can do full validation without needing to access the state of
	/// the relay-chain. Optionally provide the `TransientValidationData` which will lead to checks
	/// on the output.
	ValidateFromExhaustive(
		PersistedValidationData,
		Option<TransientValidationData>,
		ValidationCode,
		CandidateDescriptor,
		PoV,
		ResponseChannel<Result<ValidationResult>>,
	),
}
```

[NBE]: ../network.md#network-bridge-event
[AvailabilityDistributionV1NetworkMessage]: network.md#availability-distribution-v1
[BitfieldDistributionV1NetworkMessage]: network.md#bitfield-distribution-v1
[PoVDistributionV1NetworkMessage]: network.md#pov-distribution-v1
[StatementDistributionV1NetworkMessage]: network.md#statement-distribution-v1
[CollatorProtocolV1NetworkMessage]: network.md#collator-protocol-v1
