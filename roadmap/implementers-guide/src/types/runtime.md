# Runtime

Types used within the runtime exclusively and pervasively.

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
	pub group_rotation_frequency: BlockNumber,
	/// The availability period, in blocks, for parachains. This is the amount of blocks
	/// after inclusion that validators have to make the block available and signal its availability to
	/// the chain. Must be at least 1.
	pub chain_availability_period: BlockNumber,
	/// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
	/// but a differing timeout due to differing requirements. Must be at least 1.
	pub thread_availability_period: BlockNumber,
	/// The amount of blocks ahead to schedule parathreads.
	pub scheduling_lookahead: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub watermark_upward_queue_size: u32,
}
```
