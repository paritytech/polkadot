# Collation Generation

The collation generation subsystem is executed on collator nodes and produces candidates to be distributed to validators. If configured to produce collations for a para, it produces collations and then feeds them to the [Collator Protocol][CP] subsystem, which handles the networking.

## Protocol

Collation generation for Parachains currently works in the following way:

1.  A new relay chain block is imported.
2.  The collation generation subsystem checks if the core associated to
    the parachain is free and if yes, continues.
3.  Collation generation calls our collator callback, if present, to generate a PoV. If none exists, do nothing.
4.  Authoring logic determines if the current node should build a PoV.
5.  Build new PoV and give it back to collation generation.

## Messages

### Incoming

- `ActiveLeaves`
  - Notification of a change in the set of active leaves.
  - Triggers collation generation procedure outlined in "Protocol" section.
- `CollationGenerationMessage::Initialize`
  - Initializes the subsystem. Carries a config.
  - No more than one initialization message should ever be sent to the collation
    generation subsystem.
  - Sent by a collator to initialize this subsystem.
- `CollationGenerationMessage::SubmitCollation`
  - If the subsystem isn't initialized or the relay-parent is too old to be relevant, ignore the message.
  - Otherwise, use the provided parameters to generate a [`CommittedCandidateReceipt`]
  - Submit the collation to the collator-protocol with `CollatorProtocolMessage::DistributeCollation`.

### Outgoing

- `CollatorProtocolMessage::DistributeCollation`
  - Provides a generated collation to distribute to validators.

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
  pub collator: Option<CollatorFn>,
  /// The parachain that this collator collates for
  pub para_id: ParaId,
}
```

The configuration should be optional, to allow for the case where the node is not run with the capability to collate.

### Summary in plain English

- **Collation (output of a collator)**

  - Contains the PoV (proof to verify the state transition of the
    parachain) and other data.

- **Collation result**

  - Contains the collation, and an optional result sender for a
    collation-seconded signal.

- **Collation seconded signal**

  - The signal that is returned when a collation was seconded by a
    validator.

- **Collation function**

  - Called with the relay chain block the parablock will be built on top
    of.
  - Called with the validation data.
    - Provides information about the state of the parachain on the relay
      chain.

- **Collation generation config**

  - Contains collator's authentication key, optional collator function, and
    parachain ID.

[CP]: collator-protocol.md
