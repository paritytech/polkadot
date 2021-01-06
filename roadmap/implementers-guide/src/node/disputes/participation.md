# Dispute Participation

Tracks all open disputes in the active leaves set.

## Remarks

`UnsignedTransaction`s are OK to be used, since the inner
element `CommittedCandidateReceipt` is verifiable.

## Assumptions

* If each validator gossips its own vote regarding the disputed block, upon receiving the first vote (the initiating dispute vote) there is only
a need for a very small gossip only containing the `Vote` itself.

## IO

Inputs:

* `DisputeEvent::DisputeVote`
* `DisputeEvent::DisputeResolution`

Outputs:

* `X::DoubleVoteDetected`
* `VotesDbMessage::StoreVote`
* `AvailabilityRecoveryMessage::RecoverAvailableData` recover the `PoV` from the availability store, in order to verify correctness
* `CandidateValidationMessage::ValidateFromChainState` verify correctness using runtime validation code

## Helper Structs

```rust
pub enum Vote {
	/// Fragment information of a `BackedCandidate`.
	Backing {
		/// The required checkable signature for the statement.
		attestation: ValidityAttestation,
		/// Validator index of the backing validator in the validator set
		/// back then in that session.
		validator_index: ValidatorIndex,
		/// Committed candidate receipt for the candidate.
		candidate_receipt: CommittedCandidateReceipt,
	},
	/// Result of secondary checking the dispute block.
	ApprovalCheck {
		/// Signed full statement to back the vote.
		sfs: SignedFullStatement
	},
	/// An explicit vote on the disputed candidate.
	DisputePositive {
		/// Signed full statement to back the vote.
		sfs: SignedFullStatement
	},
	/// An explicit vote on the disputed candidate.
	DisputeNegative {
		/// Signed full statement to back the vote.
		sfs: SignedFullStatement
	},
}
```

```rust
/// Resolution of a vote:
enum Resolution {
    /// The block is valid
    Valid,
    /// The block is valid
    Invalid,
}
```

## Constants

* `DISPUTE_POST_RESOLUTION_TIME = Duration::from_secs(24 * 60 * 60)`: Keep a dispute open for an additional time where validators
can cast their vote.

## Session Change

Operation resumes, since the validator set at the time of the dispute
stays operational, and the validators are still bonded.

## Storage

Nothing to store, everything lives in `VotesDB`.

## Sequences

### ViewChange is BlockImport

1. Extract imported blocks from `ViewChange`
1. for each imported block:
	2. Extract the associated dispute _runtime_ events
	3. for each runtime event:
		1. Store vote according to the runtime events to the votesdb via `VotesDBMessage::StoreVote`
		1. iff: result is `AlreadyPresent`
			1. nop
		1. else iff: result is `DoubleVote`
			1. send `X::DoubleVote` to the overseer
		1. else iff: result is `Resolution`
			1. start timeout for late messages
			1. add `DisputeVote` event to transaction events
			1. craft unsigned transaction with transaction events
			1. add to transaction pool via `submit_and_watch`
		1. else:
			1. iff: result is `Detection`
				1. Request `AvailableData` via `RecoverAvailableData` to obtain the `PoV`.
				1. Call `ValidateFromChainState` to validate the `PoV` with the runtime validation logic
			1. else iff: result is `Resolution`
				1. start post resolution timeout
			1. else iff: result is `Stored`
				1. nop

			1. add `DisputeVote` event to transaction events
			1. craft unsigned transaction with transaction events
			1. add to transaction pool via `submit_and_watch`
				1. `TransactionSource::Local`
				1. `Extrinsic::IncludeData(scale_encoded)`
