# Dispute Participation

This subsystem is responsible for actually participating in disputes: when notified of a dispute, we need to recover the candidate data, validate the candidate, and cast our vote in the dispute.

Fortunately, most of that work is handled by other subsystems; this subsystem is just a small glue component for tying other subsystems together and issuing statements based on their validity.

## Protocol

Input: [DisputeParticipationMessage][DisputeParticipationMessage]

Output:
  - [RuntimeApiMessage][RuntimeApiMessage]
  - [CandidateValidationMessage][CandidateValidationMessage]
  - [AvailabilityRecoveryMessage][AvailabilityRecoveryMessage]
  - [ChainApiMessage][ChainApiMessage]

## Functionality

In-memory state:

```rust
struct State {
    recent_block_hash: Hash
}
```

### On `OverseerSignal::ActiveLeavesUpdate`

Do nothing.

### On `OverseerSignal::BlockFinalized`

Do nothing.

### On `OverseerSignal::Conclude`

Conclude.

### On `DisputeParticipationMessage::Participate`

> TODO: this validation code fetching procedure is not helpful for disputed blocks that are in chains we do not know. After https://github.com/paritytech/polkadot/issues/2457 we should use the `ValidationCodeByHash` runtime API using the code hash in the candidate receipt.

* Decompose into parts: `{ candidate_hash, candidate_receipt, session, voted_indices }`
* Issue an [`AvailabilityRecoveryMessage::RecoverAvailableData`][AvailabilityRecoveryMessage]
* If the result is `Unavailable`, return.
* If the result is `Invalid`, [cast invalid votes](#cast-votes) and return.
* Fetch the block number of `candidate_receipt.descriptor.relay_parent` using a [`ChainApiMessage::BlockNumber`][ChainApiMessage].
* If the data is recovered, dispatch a [`RuntimeApiMessage::HistoricalValidationCode`][RuntimeApiMessage] with the parameters `(candidate_receipt.descriptor.para_id, relay_parent_number)`.
* Dispatch a [`AvailabilityStoreMessage::StoreAvailableData`][AvailabilityStoreMessage] with the data.
* If the code is not fetched from the chain, return. This should be impossible with correct relay chain configuration after the TODO above is addressed and is unlikely before then, at least if chain synchronization is working correctly.
* Dispatch a [`CandidateValidationMessage::ValidateFromExhaustive`][CandidateValidationMessage] with the available data and the validation code.
* If the validation result is `Invalid`, [cast invalid votes](#cast-votes) and return.
* If the validation fails, [cast invalid votes](#cast-votes) and return.
* If the validation succeeds, compute the `CandidateCommitments` based on the validation result and compare against the candidate receipt's `commitments_hash`. If they match, [cast valid votes](#cast-votes) and if not, [cast invalid votes](#cast-votes).

### Cast Votes

This requires the parameters `{ candidate_receipt, candidate_hash, session, voted_indices }` as well as a choice of either `Valid` or `Invalid`.

Invoke [`DisputeCoordinatorMessage::IssueLocalStatement`][DisputeCoordinatorMessage] with `is_valid` according to the parameterization.

Invoke [`DisputeCoordinatorMessage::ImportStatements`][DisputeCoordinatorMessage] with each signed statement.

[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
[DisputeCoordinatorMessage]: ../../types/overseer-protocol.md#dispute-coordinator-message
[CandidateValidationMessage]: ../../types/overseer-protocol.md#candidate-validation-message
[AvailabilityRecoveryMessage]: ../../types/overseer-protocol.md#availability-recovery-message
[ChainApiMessage]: ../../types/overseer-protocol.md#chain-api-message
[AvailabilityStoreMessage]: ../../types/overseer-protocol.md#availability-store-message
