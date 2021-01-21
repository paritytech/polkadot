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
  /// Messages destined to be interpreted by the Relay chain itself.
  pub upward_messages: Vec<UpwardMessage>,
  /// New validation code.
  pub new_validation_code: Option<ValidationCode>,
  /// The head-data produced as a result of execution.
  pub head_data: HeadData,
  /// Proof to verify the state transition of the parachain.
  pub proof_of_validity: PoV,
}

type CollatorFn = Box<
  dyn Fn(Hash, &PeristedValidationData) -> Pin<Box<dyn Future<Output = Option<Collation>>>>
>;

struct CollationGenerationConfig {
  key: CollatorPair,
  /// Collate will be called with the relay chain hash the parachain should build
  /// a block on and the `ValidationData` that provides information about the state
  /// of the parachain on the relay chain.
  collator: CollatorFn,
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
