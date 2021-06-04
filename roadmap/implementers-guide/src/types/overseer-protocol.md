# Overseer Protocol

This chapter contains message types sent to and from the overseer, and the underlying subsystem message types that are transmitted using these.

## Overseer Signal

Signals from the overseer to a subsystem to request change in execution that has to be obeyed by the subsystem.

```rust
enum OverseerSignal {
  /// Signal about a change in active leaves.
  ActiveLeavesUpdate(ActiveLeavesUpdate),
  /// Signal about a new best finalized block.
  BlockFinalized(Hash),
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
enum LeafStatus {
    // A leaf is fresh when it's the first time the leaf has been encountered.
    // Most leaves should be fresh.
    Fresh,
    // A leaf is stale when it's encountered for a subsequent time. This will
    // happen when the chain is reverted or the fork-choice rule abandons some
    // chain.
    Stale,
}

struct ActiveLeavesUpdate {
    activated: [(Hash, Number, LeafStatus)], // in practice, these should probably be a SmallVec
    deactivated: [Hash],
}
```

## Approval Voting

Messages received by the approval voting subsystem.

```rust
enum AssignmentCheckResult {
    // The vote was accepted and should be propagated onwards.
    Accepted,
    // The vote was valid but duplicate and should not be propagated onwards.
    AcceptedDuplicate,
    // The vote was valid but too far in the future to accept right now.
    TooFarInFuture,
    // The vote was bad and should be ignored, reporting the peer who propagated it.
    Bad,
}

enum ApprovalCheckResult {
    // The vote was accepted and should be propagated onwards.
    Accepted,
    // The vote was bad and should be ignored, reporting the peer who propagated it.
    Bad,
}

enum ApprovalVotingMessage {
    /// Check if the assignment is valid and can be accepted by our view of the protocol.
    /// Should not be sent unless the block hash is known.
    CheckAndImportAssignment(
        IndirectAssignmentCert,
        CandidateIndex, // The index of the candidate included in the block.
        ResponseChannel<AssignmentCheckResult>,
    ),
    /// Check if the approval vote is valid and can be accepted by our view of the
    /// protocol.
    ///
    /// Should not be sent unless the block hash within the indirect vote is known.
    CheckAndImportApproval(
        IndirectSignedApprovalVote,
        ResponseChannel<ApprovalCheckResult>,
    ),
    /// Returns the highest possible ancestor hash of the provided block hash which is
    /// acceptable to vote on finality for. Along with that, return the lists of candidate hashes
    /// which appear in every block from the (non-inclusive) base number up to (inclusive) the specified
    /// approved ancestor.
    /// This list starts from the highest block (the approved ancestor itself) and moves backwards
    /// towards the base number.
    ///
    /// The base number is typically the number of the last finalized block, but in GRANDPA it is
    /// possible for the base to be slightly higher than the last finalized block.
    ///
    /// The `BlockNumber` provided is the number of the block's ancestor which is the
    /// earliest possible vote.
    ///
    /// It can also return the same block hash, if that is acceptable to vote upon.
    /// Return `None` if the input hash is unrecognized.
    ApprovedAncestor {
        target_hash: Hash,
        base_number: BlockNumber,
        rx: ResponseChannel<Option<(Hash, BlockNumber, Vec<(Hash, Vec<CandidateHash>)>)>>
    },
}
```

## Approval Distribution

Messages received by the approval Distribution subsystem.

```rust
/// Metadata about a block which is now live in the approval protocol.
struct BlockApprovalMeta {
    /// The hash of the block.
    hash: Hash,
    /// The number of the block.
    number: BlockNumber,
    /// The candidates included by the block. Note that these are not the same as the candidates that appear within the
    /// block body.
    parent_hash: Hash,
    /// The candidates included by the block. Note that these are not the same as the candidates that appear within the
    /// block body.
    candidates: Vec<CandidateHash>,
    /// The consensus slot of the block.
    slot: Slot,
}

enum ApprovalDistributionMessage {
    /// Notify the `ApprovalDistribution` subsystem about new blocks and the candidates contained within
    /// them.
    NewBlocks(Vec<BlockApprovalMeta>),
    /// Distribute an assignment cert from the local validator. The cert is assumed
    /// to be valid, relevant, and for the given relay-parent and validator index.
    ///
    /// The `u32` param is the candidate index in the fully-included list.
    DistributeAssignment(IndirectAssignmentCert, u32),
    /// Distribute an approval vote for the local validator. The approval vote is assumed to be
    /// valid, relevant, and the corresponding approval already issued. If not, the subsystem is free to drop
    /// the message.
    DistributeApproval(IndirectSignedApprovalVote),
    /// An update from the network bridge.
    NetworkBridgeUpdateV1(NetworkBridgeEvent<ApprovalDistributionV1Message>),
}
```

## All Messages

> TODO (now)

## Availability Distribution Message

Messages received by the availability distribution subsystem.

This is a network protocol that receives messages of type [`AvailabilityDistributionV1Message`][AvailabilityDistributionV1NetworkMessage].

```rust
enum AvailabilityDistributionMessage {
      /// Incoming network request for an availability chunk.
      ChunkFetchingRequest(IncomingRequest<req_res_v1::ChunkFetchingRequest>),
      /// Incoming network request for a seconded PoV.
      PoVFetchingRequest(IncomingRequest<req_res_v1::PoVFetchingRequest>),
      /// Instruct availability distribution to fetch a remote PoV.
      ///
      /// NOTE: The result of this fetch is not yet locally validated and could be bogus.
      FetchPoV {
          /// The relay parent giving the necessary context.
          relay_parent: Hash,
          /// Validator to fetch the PoV from.
          from_validator: ValidatorIndex,
          /// Candidate hash to fetch the PoV for.
          candidate_hash: CandidateHash,
          /// Expected hash of the PoV, a PoV not matching this hash will be rejected.
          pov_hash: Hash,
          /// Sender for getting back the result of this fetch.
          ///
          /// The sender will be canceled if the fetching failed for some reason.
          tx: oneshot::Sender<PoV>,
      },
}
```

## Availability Recovery Message

Messages received by the availability recovery subsystem.

```rust
enum RecoveryError {
    Invalid,
    Unavailable,
}
enum AvailabilityRecoveryMessage {
    /// Recover available data from validators on the network.
    RecoverAvailableData(
        CandidateReceipt,
        SessionIndex,
        Option<GroupIndex>, // Backing validator group to request the data directly from.
        ResponseChannel<Result<AvailableData, RecoveryError>>,
    ),
}
```

## Availability Store Message

Messages to and from the availability store.

```rust
enum AvailabilityStoreMessage {
    /// Query the `AvailableData` of a candidate by hash.
    QueryAvailableData(CandidateHash, ResponseChannel<Option<AvailableData>>),
    /// Query whether an `AvailableData` exists within the AV Store.
    QueryDataAvailability(CandidateHash, ResponseChannel<bool>),
    /// Query a specific availability chunk of the candidate's erasure-coding by validator index.
    /// Returns the chunk and its inclusion proof against the candidate's erasure-root.
    QueryChunk(CandidateHash, ValidatorIndex, ResponseChannel<Option<ErasureChunk>>),
    /// Query all chunks that we have locally for the given candidate hash.
    QueryAllChunks(CandidateHash, ResponseChannel<Vec<ErasureChunk>>),
    /// Store a specific chunk of the candidate's erasure-coding by validator index, with an
    /// accompanying proof.
    StoreChunk(CandidateHash, ErasureChunk, ResponseChannel<Result<()>>),
    /// Store `AvailableData`. If `ValidatorIndex` is provided, also store this validator's
    /// `ErasureChunk`.
    StoreAvailableData(CandidateHash, Option<ValidatorIndex>, u32, AvailableData, ResponseChannel<Result<()>>),
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
  GetBackedCandidates(Hash, Vec<CandidateHash>, ResponseChannel<Vec<BackedCandidate>>),
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
    /// A candidate collation can be fetched from a collator and should be considered for seconding.
    Collation(RelayParent, ParaId, CollatorId),
    /// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
    Invalid(RelayParent, CandidateReceipt),
    /// The candidate we recommended to be seconded was validated successfully.
    Seconded(RelayParent, SignedFullStatement),
}
```

## Chain API Message

The Chain API subsystem is responsible for providing an interface to chain data.

```rust
enum ChainApiMessage {
    /// Get the block number by hash.
    /// Returns `None` if a block with the given hash is not present in the db.
    BlockNumber(Hash, ResponseChannel<Result<Option<BlockNumber>, Error>>),
    /// Request the block header by hash.
    /// Returns `None` if a block with the given hash is not present in the db.
    BlockHeader(Hash, ResponseChannel<Result<Option<BlockHeader>, Error>>),
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
    /// Provide a collation to distribute to validators with an optional result sender.
    ///
    /// The result sender should be informed when at least one parachain validator seconded the collation. It is also
    /// completely okay to just drop the sender.
    DistributeCollation(CandidateReceipt, PoV, Option<oneshot::Sender<SignedFullStatement>>),
    /// Fetch a collation under the given relay-parent for the given ParaId.
    FetchCollation(Hash, ParaId, ResponseChannel<(CandidateReceipt, PoV)>),
    /// Report a collator as having provided an invalid collation. This should lead to disconnect
    /// and blacklist of the collator.
    ReportCollator(CollatorId),
    /// Note a collator as having provided a good collation.
    NoteGoodCollation(CollatorId, SignedFullStatement),
    /// Notify a collator that its collation was seconded.
    NotifyCollationSeconded(CollatorId, Hash, SignedFullStatement),
}
```

## Dispute Coordinator Message

Messages received by the [Dispute Coordinator subsystem](../node/disputes/dispute-coordinator.md)

This subsystem coordinates participation in disputes, tracks live disputes, and observed statements of validators from subsystems.

```rust
enum DisputeCoordinatorMessage {
    /// Import a statement by a validator about a candidate.
    ///
    /// The subsystem will silently discard ancient statements or sets of only dispute-specific statements for
    /// candidates that are previously unknown to the subsystem. The former is simply because ancient
    /// data is not relevant and the latter is as a DoS prevention mechanism. Both backing and approval
    /// statements already undergo anti-DoS procedures in their respective subsystems, but statements
    /// cast specifically for disputes are not necessarily relevant to any candidate the system is
    /// already aware of and thus present a DoS vector. Our expectation is that nodes will notify each
    /// other of disputes over the network by providing (at least) 2 conflicting statements, of which one is either
    /// a backing or validation statement.
    ///
    /// This does not do any checking of the message signature.
    ImportStatements {
        /// The hash of the candidate.
        candidate_hash: CandidateHash,
        /// The candidate receipt itself.
        candidate_receipt: CandidateReceipt,
        /// The session the candidate appears in.
        session: SessionIndex,
        /// Triples containing the following:
        /// - A statement, either indicating validity or invalidity of the candidate.
        /// - The validator index (within the session of the candidate) of the validator casting the vote.
        /// - The signature of the validator casting the vote.
        statements: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
    },
    /// Fetch a list of all active disputes that the co-ordinator is aware of.
    ActiveDisputes(ResponseChannel<Vec<(SessionIndex, CandidateHash)>>),
    /// Get candidate votes for a candidate.
    QueryCandidateVotes(SessionIndex, CandidateHash, ResponseChannel<Option<CandidateVotes>>),
    /// Sign and issue local dispute votes. A value of `true` indicates validity, and `false` invalidity.
    IssueLocalStatement(SessionIndex, CandidateHash, CandidateReceipt, bool),
    /// Determine the highest undisputed block within the given chain, based on where candidates
    /// were included. If even the base block should not be finalized due to a dispute,
    /// then `None` should be returned on the channel.
    ///
    /// The block descriptions begin counting upwards from the block after the given `base_number`. The `base_number`
    /// is typically the number of the last finalized block but may be slightly higher. This block
    /// is inevitably going to be finalized so it is not accounted for by this function.
    DetermineUndisputedChain {
        base_number: BlockNumber,
        block_descriptions: Vec<(BlockHash, SessionIndex, Vec<CandidateHash>)>,
        rx: ResponseSender<Option<(BlockNumber, BlockHash)>>,
    }
}
```

## Dispute Participation Message

Messages received by the [Dispute Participation subsystem](../node/disputes/dispute-participation.md)

This subsystem simply executes requests to evaluate a candidate.

```rust
enum DisputeParticipationMessage {
    /// Validate a candidate for the purposes of participating in a dispute.
    Participate {
        /// The hash of the candidate
        candidate_hash: CandidateHash,
        /// The candidate receipt itself.
        candidate_receipt: CandidateReceipt,
        /// The session the candidate appears in.
        session: SessionIndex,
        /// The indices of validators who have already voted on this candidate.
        voted_indices: Vec<ValidatorIndex>,
    }
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
    ReportPeer(PeerId, cost_benefit: i32),
    /// Disconnect a peer from the given peer-set without affecting their reputation.
    DisconnectPeer(PeerId, PeerSet),
    /// Send a message to one or more peers on the validation peerset.
    SendValidationMessage([PeerId], ValidationProtocolV1),
    /// Send a message to one or more peers on the collation peerset.
    SendCollationMessage([PeerId], ValidationProtocolV1),
    /// Send multiple validation messages.
    SendValidationMessages([([PeerId, ValidationProtocolV1])]),
    /// Send multiple collation messages.
    SendCollationMessages([([PeerId, ValidationProtocolV1])]),
    /// Connect to peers who represent the given `validator_ids`.
    ///
    /// Also ask the network to stay connected to these peers at least
    /// until a new request is issued.
    ///
    /// Because it overrides the previous request, it must be ensured
    /// that `validator_ids` include all peers the subsystems
    /// are interested in (per `PeerSet`).
    ///
    /// A caller can learn about validator connections by listening to the
    /// `PeerConnected` events from the network bridge.
    ConnectToValidators {
        /// Ids of the validators to connect to.
        validator_ids: Vec<AuthorityDiscoveryId>,
        /// The underlying protocol to use for this request.
        peer_set: PeerSet,
        /// Sends back the number of `AuthorityDiscoveryId`s which
        /// authority discovery has failed to resolve.
        failed: oneshot::Sender<usize>,
    },
}
```

## Misbehavior Report

```rust
pub type Misbehavior = generic::Misbehavior<
    CommittedCandidateReceipt,
    CandidateHash,
    ValidatorIndex,
    ValidatorSignature,
>;

mod generic {
    /// Misbehavior: voting more than one way on candidate validity.
    ///
    /// Since there are three possible ways to vote, a double vote is possible in
    /// three possible combinations (unordered)
    pub enum ValidityDoubleVote<Candidate, Digest, Signature> {
        /// Implicit vote by issuing and explicitly voting validity.
        IssuedAndValidity((Candidate, Signature), (Digest, Signature)),
        /// Implicit vote by issuing and explicitly voting invalidity
        IssuedAndInvalidity((Candidate, Signature), (Digest, Signature)),
        /// Direct votes for validity and invalidity
        ValidityAndInvalidity(Candidate, Signature, Signature),
    }

    /// Misbehavior: multiple signatures on same statement.
    pub enum DoubleSign<Candidate, Digest, Signature> {
        /// On candidate.
        Candidate(Candidate, Signature, Signature),
        /// On validity.
        Validity(Digest, Signature, Signature),
        /// On invalidity.
        Invalidity(Digest, Signature, Signature),
    }

    /// Misbehavior: declaring multiple candidates.
    pub struct MultipleCandidates<Candidate, Signature> {
        /// The first candidate seen.
        pub first: (Candidate, Signature),
        /// The second candidate seen.
        pub second: (Candidate, Signature),
    }

    /// Misbehavior: submitted statement for wrong group.
    pub struct UnauthorizedStatement<Candidate, Digest, AuthorityId, Signature> {
        /// A signed statement which was submitted without proper authority.
        pub statement: SignedStatement<Candidate, Digest, AuthorityId, Signature>,
    }

    pub enum Misbehavior<Candidate, Digest, AuthorityId, Signature> {
        /// Voted invalid and valid on validity.
        ValidityDoubleVote(ValidityDoubleVote<Candidate, Digest, Signature>),
        /// Submitted multiple candidates.
        MultipleCandidates(MultipleCandidates<Candidate, Signature>),
        /// Submitted a message that was unauthorized.
        UnauthorizedStatement(UnauthorizedStatement<Candidate, Digest, AuthorityId, Signature>),
        /// Submitted two valid signatures for the same message.
        DoubleSign(DoubleSign<Candidate, Digest, Signature>),
    }
}
```

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
  BackedCandidate(CandidateReceipt),
  /// Misbehavior reports are self-contained proofs of validator misbehavior.
  MisbehaviorReport(Hash, MisbehaviorReport),
  /// Disputes trigger a broad dispute resolution process.
  Dispute(Hash, Signature),
}

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
enum ProvisionerMessage {
  /// This message allows external subsystems to request current inherent data that could be used for
  /// advancing the state of parachain consensus in a block building upon the given hash.
  ///
  /// If called at different points in time, this may give different results.
  RequestInherentData(Hash, oneshot::Sender<ParaInherentData>),
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
    /// Get information about all availability cores.
    AvailabilityCores(ResponseChannel<Vec<CoreState>>),
    /// with the given occupied core assumption.
    PersistedValidationData(
        ParaId,
        OccupiedCoreAssumption,
        ResponseChannel<Option<PersistedValidationData>>,
    ),
    /// Sends back `true` if the commitments pass all acceptance criteria checks.
    CheckValidationOutputs(
        ParaId,
        CandidateCommitments,
        RuntimeApiSender<bool>,
    ),
    /// Get the session index for children of the block. This can be used to construct a signing
    /// context.
    SessionIndexForChild(ResponseChannel<SessionIndex>),
    /// Get the validation code for a specific para, using the given occupied core assumption.
    ValidationCode(ParaId, OccupiedCoreAssumption, ResponseChannel<Option<ValidationCode>>),
    /// Fetch the historical validation code used by a para for candidates executed in
    /// the context of a given block height in the current chain.
    HistoricalValidationCode(ParaId, BlockNumber, ResponseChannel<Option<ValidationCode>>),
    /// Get a committed candidate receipt for all candidates pending availability.
    CandidatePendingAvailability(ParaId, ResponseChannel<Option<CommittedCandidateReceipt>>),
    /// Get all events concerning candidates in the last block.
    CandidateEvents(ResponseChannel<Vec<CandidateEvent>>),
    /// Get the session info for the given session, if stored.
    SessionInfo(SessionIndex, ResponseChannel<Option<SessionInfo>>),
    /// Get all the pending inbound messages in the downward message queue for a para.
    DmqContents(ParaId, ResponseChannel<Vec<InboundDownwardMessage<BlockNumber>>>),
    /// Get the contents of all channels addressed to the given recipient. Channels that have no
    /// messages in them are also included.
    InboundHrmpChannelsContents(ParaId, ResponseChannel<BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>>),
    /// Get information about the BABE epoch this block was produced in.
    BabeEpoch(ResponseChannel<BabeEpoch>),
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
    /// Candidate is valid, and here are the outputs and the validation data used to form inputs.
    /// In practice, this should be a shared type so that validation caching can be done.
    Valid(CandidateCommitments, PersistedValidationData),
    /// Candidate is invalid.
    Invalid,
}

/// Messages received by the Validation subsystem.
///
/// ## Validation Requests
///
/// Validation requests made to the subsystem should return an error only on internal error.
/// Otherwise, they should return either `Ok(ValidationResult::Valid(_))`
/// or `Ok(ValidationResult::Invalid)`.
#[derive(Debug)]
pub enum CandidateValidationMessage {
    /// Validate a candidate with provided parameters using relay-chain state.
    ///
    /// This will implicitly attempt to gather the `PersistedValidationData` and `ValidationCode`
    /// from the runtime API of the chain, based on the `relay_parent`
    /// of the `CandidateDescriptor`.
    ///
    /// This will also perform checking of validation outputs against the acceptance criteria.
    ///
    /// If there is no state available which can provide this data or the core for
    /// the para is not free at the relay-parent, an error is returned.
    ValidateFromChainState(
        CandidateDescriptor,
        Arc<PoV>,
        oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
    ),
    /// Validate a candidate with provided, exhaustive parameters for validation.
    ///
    /// Explicitly provide the `PersistedValidationData` and `ValidationCode` so this can do full
    /// validation without needing to access the state of the relay-chain.
    ///
    /// This request doesn't involve acceptance criteria checking, therefore only useful for the
    /// cases where the validity of the candidate is established. This is the case for the typical
    /// use-case: secondary checkers would use this request relying on the full prior checks
    /// performed by the relay-chain.
    ValidateFromExhaustive(
        PersistedValidationData,
        ValidationCode,
        CandidateDescriptor,
        Arc<PoV>,
        oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
    ),
}
```

[NBE]: ../network.md#network-bridge-event
[AvailabilityDistributionV1NetworkMessage]: network.md#availability-distribution-v1
[BitfieldDistributionV1NetworkMessage]: network.md#bitfield-distribution-v1
[PoVDistributionV1NetworkMessage]: network.md#pov-distribution-v1
[StatementDistributionV1NetworkMessage]: network.md#statement-distribution-v1
[CollatorProtocolV1NetworkMessage]: network.md#collator-protocol-v1
