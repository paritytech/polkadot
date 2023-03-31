# Collation Generation

The collation generation subsystem is executed on collator nodes and produces candidates to be distributed to validators. If configured to produce collations for a para, it produces collations and then feeds them to the [Collator Protocol][CP] subsystem, which handles the networking.

## Protocol

Collation generation for Parachains currently works in the following way:

1.  A new relay chain block is imported.
2.  The collation generation subsystem checks if the core associated to
    the parachain is free and if yes, continues.
3.  Collation generation calls our collator callback to generate a PoV.
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
  pub collator: CollatorFn,
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

  - Contains collator's authentication key, collator function, and
    parachain ID.

## With Async Backing

### Protocol

- This will be more complicated as block production isn't bound to
  importing a relay chain block anymore.

- Parachains will build new blocks in fixed time frames as standalone
  chains are doing this, e.g. every 6 seconds.

- To support this we will need to separate the logic that determines
  when to build a block, from the logic that determines on which relay
  chain block to build.

### When to build

- For determining on when to build a new block we can reuse the slots
  logic from Substrate.
- We will let it run with the requested slot duration of the Parachain.
- Then we will implement a custom `SlotWorker`.
  - Every time this slot worker is triggered we will need to trigger
    some logic to determine the next relay chain block to build on top
    of.
  - It will return the relay chain block in which context the block
    should be built on, and the parachain block to build on top of.

### On which relay block to build

- This logic should be generic and should support sync / async backing.
- For **synchronous backing** we will check the best relay chain block
  to check if the core of our parachain is free.
  - The parachain slot should be calculated based on the timestamp and
    this should be calculated using `relay_chain_slot * slot_duration`.
- For **asynchronous backing** we will be more free to choose the block
  to build on, as we can also build on older relay chain blocks as well.
  - We will probably need some kind of runtime api for the Parachain to
    check if we want to build on a given relay chain block.
  - So, for example to reject building too many parachain blocks on the
    same relay chain block.
  - The parachain slot should be calculated based on the timestamp and
    this should be calculating using `relay_chain_slot * slot_duration +
     parachain_slot_duration * unincluded_segment_len`.

## Glossary

- *Slot:* Time is divided into discrete slots. Each validator in the validator
  set produces a verifiable random value, using a VRF, per slot. If below a
  threshold, this allows the validator to author a new block for that slot.

- *VRF:* Verifiable random function.

[CP]: collator-protocol.md
