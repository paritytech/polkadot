# Collation Generation

The collation generation subsystem is executed on collator nodes and produces candidates to be distributed to validators. If configured to produce collations for a para, it produces collations and then feeds them to the [Collator Protocol][CP] subsystem, which handles the networking.

## Protocol

Input: `CollationGenerationMessage`

```rust
enum CollationGenerationMessage {
  Initialize(CollationGenerationConfig),
}
```

No more than one initialization message should ever be sent to the collation generation subsystem.

Output: `CollationDistributionMessage`

## Functionality

The process of generating a collation for a parachain is very parachain-specific. As such, the details of how to do so are left beyond the scope of this description. The subsystem should be implemented as an abstract wrapper, which is aware of this configuration:

```rust
pub struct Collation {
  /// Hash of `CandidateCommitments` as understood by the collator.
  pub commitments_hash: Hash,
  pub proof_of_validity: PoV,
}

struct CollationGenerationConfig {
  key: CollatorPair,
  collator: Box<dyn Fn(&GlobalValidationData, &LocalValidationData) -> Box<dyn Future<Output = Collation>>>
  para_id: ParaId,
}
```

The configuration should be optional, to allow for the case where the node is not run with the capability to collate.

On `ActiveLeavesUpdate`:

* If there is no collation generation config, ignore.
* Otherwise, for each `activated` head in the update:
  * Determine if the para is scheduled on any core by fetching the `availability_cores` Runtime API.
    > TODO: figure out what to do in the case of occupied cores; see [this issue](https://github.com/paritytech/polkadot/issues/1573).
  * Determine an occupied core assumption to make about the para. Scheduled cores can make `OccupiedCoreAssumption::Free`.
  * Use the Runtime API subsystem to fetch the full validation data.
  * Invoke the `collator`, and use its outputs to produce a `CandidateReceipt`, signed with the configuration's `key`.
  * Dispatch a [`CollatorProtocolMessage`][CPM]`::DistributeCollation(receipt, pov)`.

[CP]: collator-protocol.md
[CPM]: ../../types/overseer-protocol.md#collatorprotocolmessage
