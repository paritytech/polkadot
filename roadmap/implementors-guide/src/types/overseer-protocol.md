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

## Candidate Selection Message

These messages are sent from the overseer to the [Candidate Selection subsystem](/node/backing/candidate-selection.html) when new parablocks are available for validation.

```rust
enum CandidateSelectionMessage {
  /// A new parachain candidate has arrived from a collator and should be considered for seconding.
  NewCandidate(PoV, ParachainBlock),
  /// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
  Invalid(CandidateReceipt),
}
```

If this subsystem chooses to second a parachain block, it dispatches a `CandidateBackingSubsystemMessage`.

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

## Validation Request Type

Various modules request that the [Candidate Validation subsystem](/node/utility/candidate-validation.html) validate a block with this message

```rust
enum PoVOrigin {
  /// The proof of validity is available here.
  Included(PoV),
  /// We need to fetch proof of validity from some peer on the network.
  Network(CandidateReceipt),
}

enum CandidateValidationMessage {
  /// Validate a candidate and issue a Statement
  Validate(CandidateHash, RelayHash, PoVOrigin),
}
```
