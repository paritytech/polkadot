# Overseer Protocol

This chapter contains message types sent to and from the overseer, and the underlying subsystem message types that are transmitted using these.

## Overseer Signal

Signals from the overseer to a subsystem to request change in execution that has to be obeyed by the subsystem.

```rust
enum OverseerSignal {
  /// Signal to start work localized to the relay-parent hash.
  StartWork(Hash),
  /// Signal to stop (or phase down) work localized to the relay-parent hash.
  StopWork(Hash),
}
```

All subsystems have their own message types; all of them need to be able to listen for overseer signals as well. There are currently two proposals for how to handle that with unified communication channels:

1. Retaining the `OverseerSignal` definition above, add `enum FromOverseer<T> {Signal(OverseerSignal), Message(T)}`.
1. Add a generic varint to `OverseerSignal`: `Message(T)`.

Either way, there will be some top-level type encapsulating messages from the overseer to each subsystem.

## All Messages

> TODO (now)

## Availability Distribution Message

Messages received by the availability distribution subsystem.

```rust
enum AvailabilityDistributionMessage {
	/// Distribute an availability chunk to other validators.
	DistributeChunk(Hash, ErasureChunk),
	/// Fetch an erasure chunk from network by candidate hash and chunk index.
	FetchChunk(Hash, u32),
	/// Event from the network.
	/// An update on network state from the network bridge.
	NetworkBridgeUpdate(NetworkBridgeEvent),
}
```

## Availability Store Message

Messages to and from the availability store.

```rust
enum AvailabilityStoreMessage {
	/// Query the PoV of a candidate by hash.
	QueryPoV(Hash, ResponseChannel<PoV>),
	/// Query a specific availability chunk of the candidate's erasure-coding by validator index.
	/// Returns the chunk and its inclusion proof against the candidate's erasure-root.
	QueryChunk(Hash, ValidatorIndex, ResponseChannel<AvailabilityChunkAndProof>),
	/// Store a specific chunk of the candidate's erasure-coding by validator index, with an
	/// accompanying proof.
	StoreChunk(Hash, ValidatorIndex, AvailabilityChunkAndProof),
}
```

## Bitfield Distribution Message

Messages received by the bitfield distribution subsystem.

```rust
enum BitfieldDistributionMessage {
	/// Distribute a bitfield signed by a validator to other validators.
	/// The bitfield distribution subsystem will assume this is indeed correctly signed.
	DistributeBitfield(relay_parent, SignedAvailabilityBitfield),
	/// Receive a network bridge update.
	NetworkBridgeUpdate(NetworkBridgeEvent),
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

## Network Bridge Message

Messages received by the network bridge. This subsystem is invoked by others to manipulate access
to the low-level networking code.

```rust
enum NetworkBridgeMessage {
	/// Register an event producer with the network bridge. This should be done early and cannot
	/// be de-registered.
	RegisterEventProducer(ProtocolId, Fn(NetworkBridgeEvent) -> AllMessages),
	/// Report a cost or benefit of a peer. Negative values are costs, positive are benefits.
	ReportPeer(PeerId, cost_benefit: i32),
	/// Send a message to one or more peers on the given protocol ID.
	SendMessage([PeerId], ProtocolId, Bytes),
}
```

## Network Bridge Update

These updates are posted from the [Network Bridge Subsystem](../node/utility/network-bridge.md) to other subsystems based on registered listeners.

```rust
struct View(Vec<Hash>); // Up to `N` (5?) chain heads.

enum NetworkBridgeEvent {
	/// A peer with given ID is now connected.
	PeerConnected(PeerId, ObservedRole), // role is one of Full, Light, OurGuardedAuthority, OurSentry
	/// A peer with given ID is now disconnected.
	PeerDisconnected(PeerId),
	/// We received a message from the given peer. Protocol ID should be apparent from context.
	PeerMessage(PeerId, Bytes),
	/// The given peer has updated its description of its view.
	PeerViewChange(PeerId, View), // guaranteed to come after peer connected event.
	/// We have posted the given view update to all connected peers.
	OurViewChange(View),
}
```


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
	NetworkBridgeUpdate(NetworkBridgeEvent),
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

Other subsystems query this data by sending these messages.

```rust
/// The information on validator groups, core assignments,
/// upcoming paras and availability cores.
struct SchedulerRoster {
	/// Validator-to-groups assignments.
	validator_groups: Vec<Vec<ValidatorIndex>>,
	/// All scheduled paras.
	scheduled: Vec<CoreAssignment>,
	/// Upcoming paras (chains and threads).
	upcoming: Vec<ParaId>,
	/// Occupied cores.
	availability_cores: Vec<Option<CoreOccupied>>,
}

enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(ResponseChannel<Vec<ValidatorId>>),
	/// Get the assignments of validators to cores, upcoming parachains.
	SchedulerRoster(ResponseChannel<SchedulerRoster>),
	/// Get a signing context for bitfields and statements.
	SigningContext(ResponseChannel<SigningContext>),
	/// Get the validation code for a specific para, assuming execution under given block number, and
	/// an optional block number representing an intermediate parablock executed in the context of
	/// that block.
	ValidationCode(ParaId, BlockNumber, Option<BlockNumber>, ResponseChannel<ValidationCode>),
}

enum RuntimeApiMessage {
	/// Make a request of the runtime API against the post-state of the given relay-parent.
	Request(Hash, RuntimeApiRequest),
}
```

## Statement Distribution Message

The Statement Distribution subsystem distributes signed statements and candidates from validators to other validators. It does this by distributing full statements, which embed the candidate receipt, as opposed to compact statements which don't.
It receives updates from the network bridge and signed statements to share with other validators.

```rust
enum StatementDistributionMessage {
	/// An update from the network bridge.
	NetworkBridgeUpdate(NetworkBridgeEvent),
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
	/// If there is no state available which can provide this data, an error is returned.
	ValidateFromChainState(CandidateDescriptor, PoV, ResponseChannel<Result<ValidationResult>>),

	/// Validate a candidate with provided parameters. Explicitly provide the `OmittedValidationData`
	/// and `ValidationCode` so this can do full validation without needing to access the state of
	/// the relay-chain.
	ValidateFromExhaustive(
		OmittedValidationData,
		ValidationCode,
		CandidateDescriptor,
		PoV,
		ResponseChannel<Result<ValidationResult>>,
	),
}
```
