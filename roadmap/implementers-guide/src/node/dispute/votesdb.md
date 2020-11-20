# Votes DB

Responsible module for tracking all kind of dispute votes by all validators
for a fixed number of sessions.


## Remarks

Votes must be persisted. Disputes might happen long after
there was a dispute, as such just keeping things in memory
is thus sufficient, as every blip (OOM, bug, maintanance)
could cause the node to forget the state.

`UnsignedTransaction`s are OK to be used, since the inner
element `CommittedCanddidateReceipt` is verifiable.

## ToDo

* Missing Definition of Governance Mode.
* Is a `PoV` enough or should the whole `CommittedCandidateReceipt` be included?
## IO

Inputs:

* `VotesDbMessage::*`

Outputs:

* `DisputeConclusionMessage::*`
* `ChainApiMessage::Blacklist`

## Messages

```rust
enum VotesDbMessage {
    /// Allow querying all `Hash`es queried by a particular validator.
    QueryByValidatorAndSessionIndex {
        identification: (SessionIndex, ValidatorIndex), response: ResponseChannel<Vec<Hash>>
    },
    QueryByValidatorId {
        identification: ValidatorId, response: ResponseChannel<Vec<Hash>>
    },

    /// Register a vote for a particular dispute
    RegisterVote(DisputeGossipMessage<H = Hash, N: BlockNumber>),
}
```

```rust
enum DisputeMessage {
    /// A dispute is detected
    Detection {
        /// unique validator indentification
        identification: (SessionIndex, ValidatorIndex),
        response: ResponseChannel<Vec<Hash>>,
    },
    /// Concluded a dispute with the following resolution
    Resolution {
        /// Resolution of the block can either be,
        /// the block being valid or not.
        valid: bool,
    }
}
```

## Helper Structs

```rust
/// Snapshot of the current vote state and recorded votes
struct Snapshot {
    /// all entries must be unique
    pro: Vec<(ValidatorIndex, SessionIndex)>,
    /// all entries must be unique
    cons: Vec<(ValidatorIndex, SessionIndex)>,
}
```

```rust
/// Gossip being sent on escalation to all other
/// relevant validators for the dispute resolution.
///
/// Currently information as provided in `BackedCandidate<H=Hash>` with
/// some extra info.
struct DisputeGossipMessage<H = Hash, N: BlockNumber> {
    /// the committed candidate receipt
    pub candidate: CommittedCandidateReceipt<H>,
    /// The block number in question
    /// TODO: not sure if this is needed given, `CandidateDescriptor`
    /// TODO: already includes sufficient identification info
    pub block_number: N,
    /// session index relevant for the block in question
    pub session_index: SessionIndex,
    /// the validity votes per validator
    pub validity_votes: Vec<ValidityAttestation>,
    /// validator indices for the particular session, which validated the
    /// disputed block.
    /// Invariant: is never all zeros/has at least one bit set
    pub backing_validators: BitVec<bitvec::order::Lsb0, u8>,
}
```

## Session Change

Nop.

## Storage

To fulfill the required query features.

```raw
(ValidatorIndex, Session) -> Vec<Hash>
(ValidatorId) -> Vec<(ValidatorIndex, Session)>
(Hash) -> (CommittedCandidateReceipt, PoV)
```

## Sequence

### Incoming message

1. Incoming request to store a dispute gossip message.
1. Validate message.
1. Check if the dispute already has a supermajority.
    1. iff:
        1. Craft and enqueue an unsigned transaction to reward/slash the validator of the proof, that is part of the incoming message, directly.
    1. else:
        1. Store the dispute including its proof to the DB.
        1. Enqueue unsigned transaction to reward/slash all validators that voted.
        1. >> Dispute Effects

### Dispute Effects

In case a block was disputed successfully, and is now deemed invalid.

1. Query `ChainApiMessage::Descendants` for all descendants of the disputed `Hash`.
1. Blacklist all descendants of the disputed block `ChainApiMessage::Blacklist`.
1. Check if the dispute block was finalized
1. iff:
    1. put the chain into governance mode


### Query

1. Incoming query request
    1. Resolve `ValidatorId` to a set of `(ValidatorIndex, SessionIndex)`.
    1. For each `(ValidatorIndex, SessionIndex)`:
        1. Accumulate `Hash`es.
    1. Lookup all `PoV`s.
