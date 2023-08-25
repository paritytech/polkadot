# Runtime

Types used within the runtime exclusively and pervasively.

## Host Configuration

The internal-to-runtime configuration of the parachain host. This is expected to be altered only by governance procedures.

```rust
struct HostConfiguration {
	/// The minimum period, in blocks, between which parachains can update their validation code.
	pub validation_upgrade_cooldown: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
	/// How long to keep code on-chain, in blocks. This should be sufficiently long that disputes
	/// have concluded.
	pub code_retention_period: BlockNumber,
	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// The amount of availability cores to dedicate to parathreads (on-demand parachains).
	pub parathread_cores: u32,
	/// The number of retries that a parathread (on-demand parachain) author has to submit their block.
	pub parathread_retries: u32,
	/// How often parachain groups should be rotated across parachains.
	pub group_rotation_frequency: BlockNumber,
	/// The availability period, in blocks, for parachains. This is the amount of blocks
	/// after inclusion that validators have to make the block available and signal its availability to
	/// the chain. Must be at least 1.
	pub chain_availability_period: BlockNumber,
	/// The availability period, in blocks, for parathreads (on-demand parachains). Same as the `chain_availability_period`,
	/// but a differing timeout due to differing requirements. Must be at least 1.
	pub thread_availability_period: BlockNumber,
	/// The amount of blocks ahead to schedule on-demand parachains.
	pub scheduling_lookahead: u32,
	/// The maximum number of validators to have per core. `None` means no maximum.
	pub max_validators_per_core: Option<u32>,
	/// The maximum number of validators to use for parachains, in total. `None` means no maximum.
	pub max_validators: Option<u32>,
	/// The amount of sessions to keep for disputes.
	pub dispute_period: SessionIndex,
	/// How long after dispute conclusion to accept statements.
	pub dispute_post_conclusion_acceptance_period: BlockNumber,
	/// The maximum number of dispute spam slots
	pub dispute_max_spam_slots: u32,
	/// The amount of consensus slots that must pass between submitting an assignment and
	/// submitting an approval vote before a validator is considered a no-show.
	/// Must be at least 1.
	pub no_show_slots: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// The width of the zeroth delay tranche for approval assignments. This many delay tranches
	/// beyond 0 are all consolidated to form a wide 0 tranche.
	pub zeroth_delay_tranche_width: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
	/// The number of samples to use in `RelayVRFModulo` or `RelayVRFModuloCompact` approval assignment criterions.
	pub relay_vrf_modulo_samples: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub max_upward_queue_size: u32,
	/// The maximum size of an upward message that can be sent by a candidate.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub max_upward_message_size: u32,
	/// The maximum number of messages that a candidate can contain.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub max_upward_message_num_per_candidate: u32,
	/// The maximum size of a message that can be put in a downward message queue.
	///
	/// Since we require receiving at least one DMP message the obvious upper bound of the size is
	/// the PoV size. Of course, there is a lot of other different things that a parachain may
	/// decide to do with its PoV so this value in practice will be picked as a fraction of the PoV
	/// size.
	pub max_downward_message_size: u32,
	/// The deposit that the sender should provide for opening an HRMP channel.
	pub hrmp_sender_deposit: u32,
	/// The deposit that the recipient should provide for accepting opening an HRMP channel.
	pub hrmp_recipient_deposit: u32,
	/// The maximum number of messages allowed in an HRMP channel at once.
	pub hrmp_channel_max_capacity: u32,
	/// The maximum total size of messages in bytes allowed in an HRMP channel at once.
	pub hrmp_channel_max_total_size: u32,
	/// The maximum number of inbound HRMP channels a parachain is allowed to accept.
	pub hrmp_max_parachain_inbound_channels: u32,
	/// The maximum number of inbound HRMP channels a parathread (on-demand parachain) is allowed to accept.
	pub hrmp_max_parathread_inbound_channels: u32,
	/// The maximum size of a message that could ever be put into an HRMP channel.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_channel_max_message_size: u32,
	/// The maximum number of outbound HRMP channels a parachain is allowed to open.
	pub hrmp_max_parachain_outbound_channels: u32,
	/// The maximum number of outbound HRMP channels a parathread (on-demand parachain) is allowed to open.
	pub hrmp_max_parathread_outbound_channels: u32,
	/// The maximum number of outbound HRMP messages can be sent by a candidate.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_max_message_num_per_candidate: u32,
}
```

## ParaInherentData

Inherent data passed to a runtime entry-point for the advancement of parachain consensus.

This contains 4 pieces of data:
1. [`Bitfields`](availability.md#signed-availability-bitfield)
2. [`BackedCandidates`](backing.md#backed-candidate)
3. [`MultiDisputeStatementSet`](disputes.md#multidisputestatementset)
4. `Header`

```rust
struct ParaInherentData {
	bitfields: Bitfields,
	backed_candidates: BackedCandidates,
	dispute_statements: MultiDisputeStatementSet,
	parent_header: Header
}
```
