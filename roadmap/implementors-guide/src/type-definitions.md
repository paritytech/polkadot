# Data Structures and Types

> TODO:
>
> * CandidateReceipt
> * CandidateCommitments
> * AbridgedCandidateReceipt
> * GlobalValidationSchedule
> * LocalValidationData (should commit to code hash too - see Remote disputes section of validity module)

## Block Import Event

```rust
/// Indicates that a new block has been added to the blockchain.
struct BlockImportEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header itself.
  header: Header,
  /// Whether this block is considered the head of the best chain according to the
  /// event emitter's fork-choice rule.
  new_best: bool,
}
```

## Block Finalization Event

```rust
/// Indicates that a new block has been finalized.
struct BlockFinalizationEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header of the finalized block.
  header: Header,
}
```

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

## Statement Type

The [Candidate Validation subsystem](/node/utility/candidate-validation.html) issues these messages in reponse to `ValidationRequest`s. The [Candidate Backing subsystem](/node/backing/candidate-backing.html) may upgrade the `Valid` variant to `Seconded`.

```rust
/// A statement about the validity of a parachain candidate.
enum Statement {
  /// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
  ///
  /// The main semantic difference between `Seconded` and `Valid` comes from the fact that every validator may
  /// second only 1 candidate; this places an upper bound on the total number of candidates whose validity
  /// needs to be checked. A validator who seconds more than 1 parachain candidate per relay head is subject
  /// to slashing.
  Seconded(CandidateReceipt),
  /// A statement about the validity of a candidate, based on candidate's hash.
  Valid(Hash),
  /// A statement about the invalidity of a candidate.
  Invalid(Hash),
}
```

## Signed Statement Type

The actual signed payload should reference only the hash of the CandidateReceipt, even in the `Seconded` case and should include
a relay parent which provides context to the signature. This prevents against replay attacks and allows the candidate receipt itself
to be omitted when checking a signature on a `Seconded` statement.

```rust
/// A signed statement.
struct SignedStatement {
  statement: Statement,
  signed: ValidatorId,
  signature: Signature
}
```

## Statement Distribution Message

```rust
enum StatementDistributionMessage {
  /// A peer has seconded a candidate and we need to double-check them
  Peer(SignedStatement),
  /// We have validated a candidate and want to share our judgment with our peers
  ///
  /// The statement distribution subsystem is responsible for signing this statement.
  Share(Statement),
}
```

## Misbehavior Arbitration Message

```rust
enum MisbehaviorArbitrationMessage {
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

## Host Configuration

The internal-to-runtime configuration of the parachain host. This is expected to be altered only by governance procedures.

```rust
struct HostConfiguration {
  /// The minimum frequency at which parachains can update their validation code.
  pub validation_upgrade_frequency: BlockNumber,
  /// The delay, in blocks, before a validation upgrade is applied.
  pub validation_upgrade_delay: BlockNumber,
  /// The acceptance period, in blocks. This is the amount of blocks after availability that validators
  /// and fishermen have to perform secondary approval checks or issue reports.
  pub acceptance_period: BlockNumber,
  /// The maximum validation code size, in bytes.
  pub max_code_size: u32,
  /// The maximum head-data size, in bytes.
  pub max_head_data_size: u32,
  /// The amount of availability cores to dedicate to parathreads.
  pub parathread_cores: u32,
  /// The number of retries that a parathread author has to submit their block.
  pub parathread_retries: u32,
  /// How often parachain groups should be rotated across parachains.
  pub parachain_rotation_frequency: BlockNumber,
  /// The availability period, in blocks, for parachains. This is the amount of blocks
  /// after inclusion that validators have to make the block available and signal its availability to
  /// the chain. Must be at least 1.
  pub chain_availability_period: BlockNumber,
  /// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
  /// but a differing timeout due to differing requirements. Must be at least 1.
  pub thread_availability_period: BlockNumber,
  /// The amount of blocks ahead to schedule parathreads.
  pub scheduling_lookahead: u32,
}
```

## Signed Availability Bitfield

A bitfield signed by a particular validator about the availability of pending candidates.

```rust
struct SignedAvailabilityBitfield {
  validator_index: ValidatorIndex,
  bitfield: Bitvec,
  signature: ValidatorSignature, // signature is on payload: bitfield ++ relay_parent ++ validator index
}

struct Bitfields(Vec<(SignedAvailabilityBitfield)>), // bitfields sorted by validator index, ascending
```

## Validity Attestation

An attestation of validity for a candidate, used as part of a backing. Both the `Seconded` and `Valid` statements are considered attestations of validity. This structure is only useful where the candidate referenced is apparent.

```rust
enum ValidityAttestation {
  /// Implicit validity attestation by issuing.
  /// This corresponds to issuance of a `Seconded` statement.
  Implicit(ValidatorSignature),
  /// An explicit attestation. This corresponds to issuance of a
  /// `Valid` statement.
  Explicit(ValidatorSignature),
}
```

## Backed Candidate

A `CandidateReceipt` along with all data necessary to prove its backing. This is submitted to the relay-chain to process and move along the candidate to the pending-availability stage.

```rust
struct BackedCandidate {
  candidate: AbridgedCandidateReceipt,
  validity_votes: Vec<ValidityAttestation>,
  // the indices of validators who signed the candidate within the group. There is no need to include
  // bit for any validators who are not in the group, so this is more compact.
  validator_indices: BitVec,
}

struct BackedCandidates(Vec<BackedCandidate>); // sorted by para-id.
```
