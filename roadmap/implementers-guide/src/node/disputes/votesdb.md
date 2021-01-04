# Votes DB

Responsible module for tracking
all kinds of votes on candidates
by all validators for a fixed number
of sessions.


## Remarks

Votes must be persisted. Disputes might happen long after
there was a vote, as such just keeping things in memory
is thus insufficient, as every blip (OOM, bug, maintanance)
could cause the node to forget the state.

## ToDo

* Missing Definition of Governance Mode.
* Is a `CommittedCandidateReceipt` enough or should the whole `PoV` be included? There was talk about detachable validator signatures.
* Currently [`fn mark_bad()`](https://github.com/paritytech/substrate/pull/6301/files#diff-8faeb5c685a8fdff428c5ec6d9102fd59e127ff69762d43045cd38e586db5559R60-R64) does not persist data.

## IO

Inputs:

* `VotesDbMessage::DisputeVote`

Outputs:

* `DisputeParticipationMessage::Detection`
* `DisputeParticipationMessage::Resolution`

## Messages

```rust
enum VotesDbMessage {
    /// Allow querying all `Hash`es queried by a particular validator.
    QueryValidatorVotes {
        /// Validator indentification.
        session: SessionIndex,
        validator: ValidatorIndex,
        response: ResponseChannel<Vec<Hash>>,
    },

    /// Store a vote for a particular dispute
    StoreVote{
        /// Unique validator indentification
        session: SessionIndex,
        validator: ValidatorIndex,
        /// Vote.
        vote: Vote,
        /// Attestation.
        attestation: ValidityAttestation,
    },
}
```

```rust
enum DisputeMessage {
    /// A dispute is detected
    Detection {
        /// unique validator indentification
        session: SessionIndex,
        validator: ValidatorIndex,
        /// The attestation.
        attestation: ValidityAttestation,
        /// response channel
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
    pro: Vec<(ValidatorIndex, SessionIndex, ValidityAttestation)>,
    /// all entries must be unique
    cons: Vec<(ValidatorIndex, SessionIndex, ValidityAttestation)>,
}
```

## Session Change

A sweep clean is to be done to remove stored attestions
of all validators that are not slashable / have no funds
bound anymore in order to save storage space.

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

### Query

1. Incoming query request
    1. Resolve `ValidatorId` to a set of `(SessionIndex, ValidatorIndex)`.
    1. For each `(SessionIndex, ValidatorIndex)`:
        1. Accumulate `Hash`es.
    1. Lookup all `PoV`s.

### Incoming dispute vote

1. Check if the dispute already has a supermajority.
1. iff:
    1. store the resolution and keep the resolution
    1. send `DisputeMessage::Resolution` to the overseer
    1. TODO when to delete stuff from the DB?
1. else:
    1. Store the new vote including its proof to the DB.
