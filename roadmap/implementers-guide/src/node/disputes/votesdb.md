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
* Currently [`fn mark_bad()`](https://github.com/paritytech/substrate/pull/6301/files#diff-8faeb5c685a8fdff428c5ec6d9102fd59e127ff69762d43045cd38e586db5559R60-R64) does not persist data, but that is a requirement.

## IO

Inputs:

* `VotesDbMessage::StoreVote`
* `VotesDbMessage::QueryVotesByValidator`

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
	StoreVote {
		/// Unique validator indentification
		session: SessionIndex,
		validator: ValidatorIndex,
		/// Vote.
		vote: Vote,
		/// Attestation.
		attestation: ValidityAttestation,
		/// Returns `true` if the passed vote is new information
		new_vote: oneshot::Sender<VoteStoreResult>
	},
}
```


## Helper Structs

```rust
pub enum VoteStoreResult {
	/// In case the stored vote was the first negative vote stored
	/// and provides all votes (which hence must be supporting the
	/// validity claim) cast up to the point.
	Detection { supporting: Vec<Vote> },
	/// The passed voted was successfully stored and did not
	/// initiate a dispute.
	Stored,
	/// An identical vote is already present.
	AlreadyPresent,
	/// Double vote was detected for the particular validator.
	DoubleVote { previous: Vote },
	/// Quorum and super majority were reached.
	Resolution,
}
```

## Session Change

A sweep clean is to be done to remove stored attestions
of all validators that are not slashable / have no funds
bound anymore in order to bound the storage space.

## Storage

To fulfill the required query features following schema is used:

```raw
(ValidatorIndex, Session) => Vec<Hash>
ValidatorId => Vec<(ValidatorIndex, Session)>
Hash => CommittedCandidateReceipt
```


## Sequence

### Incoming message

1. Incoming request to store a `Vote` via `VotesDbMessage::StoreVote`.
1. Assure the message's content is verifying against one of the validator keys of the validators that had duty at the blocks inclusion.
1. Attempt to retrieve the message that already exists
1. iff an identical message is already present:
	1. send response `VoteStoreResult::AlreadyExists`
1. else iff a different message is already present:
	1. send response `VoteStoreResult::DoubleVote`
1. else iff the dispute already has a supermajority:
	1. send response `VoteStoreResult::Resolution`
1. else:
	1. store the `Vote` to the database
	1. send response `VoteStoreResult::Stored`

### Query

1. Incoming query request
	1. Resolve `ValidatorId` to a set of `(SessionIndex, ValidatorIndex)`.
	1. For each `(SessionIndex, ValidatorIndex)`:
		1. Accumulate `Hash`es.
	1. Lookup all `Vote`s
