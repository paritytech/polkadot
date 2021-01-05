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

* `DisputeGossip::Vote` (votes of other validators)
* `DisputeParticipationMessage::Detection`
* `DisputeParticipationMessage::Resolution`

Outputs:

* `AvailabilityRecoveryMessage::RecoverAvailableData`
* `CandidateValidationMessage::ValidateFromChainState`
* `DisputeGossip::Vote` (own vote)

## Messages

```rust
enum DisputeParticipationMessage {
    Detection {
        candidate: CandidateHash,
        votes: HashMap<ValidatorId, Vote>,
    },

    Resolution{
        resolution: Resolution,
        votes: HashMap<ValidatorId, Vote>,
    },
}
```

Distribution of our own vote via gossip, but also
receive them in order to store it to `VotesDB`:

```rust
enum DisputeGossip {
    /// A vote by a validator, referenced by session
    /// index and validator index
    Vote(SessionIndex, ValidatorIndex, Vote),
}
```

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

## Session Change

> TODO

## Storage

Nothing to store, everything lives in `VotesDB`.

## Sequence

### Detection

1. Receive `DisputeParticipationMessage::Detection`
1. Request `AvailableData` via `RecoverAvailableData` to obtain the `PoV`.
1. Call `ValidateFromChainState` to validate the `PoV` with the `CandidateDescriptor`.
    1. Cast vote according to the `ValidationResult`.
    1. Store our vote to the `VotesDB` via `VotesDBMessage::StoreVote`
    1. Gossip our vote on the network.

### Resolution

1. Receive `DisputeParticipationMessage::Resolution`
1. Craft an unsigned transaction with all votes cast
  1. includes `CandidateReceipts`
  1. includes `Vote`s
1. Delete all relevant data associated with this dispute

### Blacklisting bad block and invalid decendants

In case a block was disputed successfully, and is now deemed invalid.

1. Query `ChainApiMessage::Descendants` for all descendants of the disputed `Hash`.
1. Blacklist all descendants of the disputed block `substrate::Client::unsafe_revert`.
1. Check if the dispute block was finalized
1. iff:
    1. put the chain into governance mode
1. Craft and enqueue an unsigned transaction that does the following:
    1. reward the validator of the proof
    1. slash the party or parties identified by the sender as misbehaving
