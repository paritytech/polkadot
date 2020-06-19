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

> TODO [now]

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
  /// Registers a stream listener for updates to the set of backable candidates that could be backed
  /// in a child of the given relay-parent, referenced by its hash.
  RegisterBackingWatcher(Hash, TODO),
  /// Note that the Candidate Backing subsystem should second the given candidate in the context of the
  /// given relay-parent (ref. by hash). This candidate must be validated.
  Second(Hash, CandidateReceipt),
  /// Note a peer validator's statement about a particular candidate. Disagreements about validity must be escalated
  /// to a broader check by Misbehavior Arbitration. Agreements are simply tallied until a quorum is reached.
  Statement(Statement),
}
```

## Candidate Selection Message

These messages are sent to the [Candidate Selection subsystem](../node/backing/candidate-selection.html) as a means of providing feedback on its outputs.

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

These updates are posted from the [Network Bridge Subsystem](../node/utility/network-bridge.html) to other subsystems based on registered listeners.

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
  CandidateValidityDisagreement(CandidateReceipt, Vec<SignedStatement>),
  /// I've noticed a peer contradicting itself about a particular candidate
  SelfContradiction(CandidateReceipt, SignedStatement, SignedStatement),
  /// This peer has seconded more than one parachain candidate for this relay parent head
  DoubleVote(CandidateReceipt, SignedStatement, SignedStatement),
}
```

If this subsystem chooses to second a parachain block, it dispatches a `CandidateBackingSubsystemMessage`.

## PoV Distribution

Messages received by the PoV Distribution subsystem are unspecified and highly tied to gossip.

> TODO

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

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
enum ProvisionerMessage {
  /// This message allows potential block authors to be kept updated with all new authorship data
  /// as it becomes available.
  RequestBlockAuthorshipData(Hash, Sender<ProvisionableData>),
  /// This data should become part of a relay chain block
  ProvisionableData(ProvisionableData),
}
```

## Runtime API Message

The Runtime API subsystem is responsible for providing an interface to the state of the chain's runtime.

Other subsystems query this data by sending these messages.

```rust
enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(ResponseChannel<Vec<ValidatorId>>),
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

The Statement Distribution subsystem distributes signed statements from validators to other validators.
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
	Share(Hash, SignedStatement),
}
```

## Validation Request Type

Various modules request that the [Candidate Validation subsystem](../node/utility/candidate-validation.html) validate a block with this message

```rust
enum CandidateValidationMessage {
	/// Validate a candidate with provided parameters. Returns `Err` if an only if an internal
	/// error is encountered. A bad candidate will return `Ok(false)`, while a good one will
	/// return `Ok(true)`.
	Validate(ValidationCode, CandidateReceipt, PoV, ResponseChannel<Result<bool>>),
}
```
