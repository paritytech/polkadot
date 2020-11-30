# Dispute Participation

Tracks all open disputes in the active leaves set.

## Remarks

`UnsignedTransaction`s are OK to be used, since the inner
element `CommittedCanddidateReceipt` is verifiable.

## ToDo

> TODO

## IO

Inputs:

* `DisputeParticipationMessage::Detection`
* `DisputeParticipationMessage::Resolution`

Outputs:

* `AvailabilityRecoveryMessage::RecoverAvailableData`
* `CandidateValidationMessage::ValidateFromChainState`
* `ChainAPI::Blacklist`

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

> TODO

## Sequence

> TODO
