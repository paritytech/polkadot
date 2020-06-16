# Data Structures and Types

## Block Import Event

```rust
/// Indicates that a new block has been added to the blockchain.
struct BlockImportEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header itself.
  header: Header,
  /// Whether this block is considered the head of the best chain according to the
  /// event emitter's fork-choice rule.
  new_best: bool,
}
```

## Block Finalization Event

```rust
/// Indicates that a new block has been finalized.
struct BlockFinalizationEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header of the finalized block.
  header: Header,
}
```

## Host Configuration

The internal-to-runtime configuration of the parachain host. This is expected to be altered only by governance procedures.

```rust
struct HostConfiguration {
  /// The minimum frequency at which parachains can update their validation code.
  pub validation_upgrade_frequency: BlockNumber,
  /// The delay, in blocks, before a validation upgrade is applied.
  pub validation_upgrade_delay: BlockNumber,
  /// The acceptance period, in blocks. This is the amount of blocks after availability that validators
  /// and fishermen have to perform secondary approval checks or issue reports.
  pub acceptance_period: BlockNumber,
  /// The maximum validation code size, in bytes.
  pub max_code_size: u32,
  /// The maximum head-data size, in bytes.
  pub max_head_data_size: u32,
  /// The amount of availability cores to dedicate to parathreads.
  pub parathread_cores: u32,
  /// The number of retries that a parathread author has to submit their block.
  pub parathread_retries: u32,
  /// How often parachain groups should be rotated across parachains.
  pub parachain_rotation_frequency: BlockNumber,
  /// The availability period, in blocks, for parachains. This is the amount of blocks
  /// after inclusion that validators have to make the block available and signal its availability to
  /// the chain. Must be at least 1.
  pub chain_availability_period: BlockNumber,
  /// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
  /// but a differing timeout due to differing requirements. Must be at least 1.
  pub thread_availability_period: BlockNumber,
  /// The amount of blocks ahead to schedule parathreads.
  pub scheduling_lookahead: u32,
}
```
