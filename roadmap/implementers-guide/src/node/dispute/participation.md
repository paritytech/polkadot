# Dispute Participation

Tracks all open disputes in the active leaves set.

## Remarks

`UnsignedTransaction`s are OK to be used, since the inner
element `CommittedCanddidateReceipt` is verifiable.

## ToDo

1. Specify what is being gossiped, what messages and what they must contain.


## IO

Inputs:1. Receive `DisputeParticipationMessage::Resolution`
1. Delete all relevant data associated with this dispute


* `DisputeParticipationMessage::Detection`
* `DisputeParticipationMessage::Resolution`

Outputs:

* `AvailabilityRecoveryMessage::RecoverAvailableData`
* `CandidateValidationMessage::ValidateFromChainState`
* `...::Blacklist`

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

## Helper Structs

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

### Resolution

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
1. Blacklist all descendants of the disputed block `ChainApiMessage::Blacklist`.
1. Check if the dispute block was finalized
1. iff:
    1. put the chain into governance mode
1. Craft and enqueue an unsigned transaction to reward/slash the validator of the proof, that is part of the incoming message, directly.
