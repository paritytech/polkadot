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
	pub max_upward_queue_size: u32,
	/// The amount of weight we wish to devote to the processing the dispatchable upward messages
	/// stage.
	///
	/// NOTE that this is a soft limit and could be exceeded.
	pub preferred_dispatchable_upward_messages_step_weight: u32,
	/// Any dispatchable upward message that requests more than the critical amount is rejected
	/// with `DispatchResult::CriticalWeightExceeded`.
	///
	/// The parameter value is picked up so that no dispatchable can make the block weight exceed
	/// the total budget. I.e. that the sum of `preferred_dispatchable_upward_messages_step_weight`
	/// and `dispatchable_upward_message_critical_weight` doesn't exceed the amount of weight left
	/// under a typical worst case (e.g. no upgrades, etc) weight consumed by the required phases of
	/// block execution (i.e. initialization, finalization and inherents).
	pub dispatchable_upward_message_critical_weight: u32,
	/// The maximum number of messages that a candidate can contain.
	pub max_upward_message_num_per_candidate: u32,
	/// Number of sessions after which an HRMP open channel request expires.
	pub hrmp_open_request_ttl: u32,
	/// The deposit that the sender should provide for opening an HRMP channel.
	pub hrmp_sender_deposit: u32,
	/// The deposit that the recipient should provide for accepting opening an HRMP channel.
	pub hrmp_recipient_deposit: u32,
	/// The maximum number of messages allowed in an HRMP channel at once.
	pub hrmp_channel_max_places: u32,
	/// The maximum total size of messages in bytes allowed in an HRMP channel at once.
	pub hrmp_channel_max_size: u32,
	/// The maximum number of outbound HRMP channels a parachain is allowed to open.
	pub hrmp_max_parachain_outbound_channels: u32,
	/// The maximum number of outbound HRMP channels a parathread is allowed to open.
	pub hrmp_max_parathread_outbound_channels: u32,
}
```
