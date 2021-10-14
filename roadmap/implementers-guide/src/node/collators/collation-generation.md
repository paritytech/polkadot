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
/// The output of a collator.
///
/// This differs from `CandidateCommitments` in two ways:
///
/// - does not contain the erasure root; that's computed at the Polkadot level, not at Cumulus
/// - contains a proof of validity.
pub struct Collation {
  /// Messages destined to be interpreted by the Relay chain itself.
  pub upward_messages: Vec<UpwardMessage>,
  /// The horizontal messages sent by the parachain.
  pub horizontal_messages: Vec<OutboundHrmpMessage<ParaId>>,
  /// New validation code.
  pub new_validation_code: Option<ValidationCode>,
  /// The head-data produced as a result of execution.
  pub head_data: HeadData,
  /// Proof to verify the state transition of the parachain.
  pub proof_of_validity: PoV,
  /// The number of messages processed from the DMQ.
  pub processed_downward_messages: u32,
  /// The mark which specifies the block number up to which all inbound HRMP messages are processed.
  pub hrmp_watermark: BlockNumber,
}

/// Result of the [`CollatorFn`] invocation.
pub struct CollationResult {
  /// The collation that was build.
  pub collation: Collation,
  /// An optional result sender that should be informed about a successfully seconded collation.
  ///
  /// There is no guarantee that this sender is informed ever about any result, it is completely okay to just drop it.
  /// However, if it is called, it should be called with the signed statement of a parachain validator seconding the
  /// collation.
  pub result_sender: Option<oneshot::Sender<CollationSecondedSignal>>,
}

/// Signal that is being returned when a collation was seconded by a validator.
pub struct CollationSecondedSignal {
  /// The hash of the relay chain block that was used as context to sign [`Self::statement`].
  pub relay_parent: Hash,
  /// The statement about seconding the collation.
  ///
  /// Anything else than `Statement::Seconded` is forbidden here.
  pub statement: SignedFullStatement,
}

/// Collation function.
///
/// Will be called with the hash of the relay chain block the parachain block should be build on and the
/// [`ValidationData`] that provides information about the state of the parachain on the relay chain.
///
/// Returns an optional [`CollationResult`].
pub type CollatorFn = Box<
  dyn Fn(
      Hash,
      &PersistedValidationData,
    ) -> Pin<Box<dyn Future<Output = Option<CollationResult>> + Send>>
    + Send
    + Sync,
>;

/// Configuration for the collation generator
pub struct CollationGenerationConfig {
  /// Collator's authentication key, so it can sign things.
  pub key: CollatorPair,
  /// Collation function. See [`CollatorFn`] for more details.
  pub collator: CollatorFn,
  /// The parachain that this collator collates for
  pub para_id: ParaId,
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
