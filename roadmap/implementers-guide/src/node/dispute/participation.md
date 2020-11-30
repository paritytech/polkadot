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

## Messages

```rust
enum DisputeParticipationMessage {
    Detection {
        // TODO
    },

    Resolution{
        // TODO
    },
}
```

## Helper Structs

> TODO

## Session Change

> TODO

## Storage

Nothing to store

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
