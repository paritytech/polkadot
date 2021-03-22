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

/// Result of the [`CollatorFn`] invocation.
pub struct CollationResult {
	/// The collation that was build.
	collation: Collation,
	/// An optional result sender that should be informed about a successfully seconded collation.
	///
	/// There is no guarantee that this sender is informed ever about any result, it is completly okay to just drop it.
	/// However, if it is called, it should be called with the signed statement of a parachain validator seconding the
	/// collation.
	result_sender: Option<oneshot::Sender<SignedFullStatement>>,
}

/// Collation function.
///
/// Will be called with the hash of the relay chain block the parachain block should be build on and the
/// [`ValidationData`] that provides information about the state of the parachain on the relay chain.
///
/// Returns an optional [`CollationResult`].
pub type CollatorFn = Box<
	dyn Fn(Hash, &PersistedValidationData) -> Pin<Box<dyn Future<Output = Option<CollationResult>> + Send>>
		+ Send
		+ Sync,
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
