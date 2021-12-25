// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Configuration manager for the Polkadot runtime parachains logic.
//!
//! Configuration can change only at session boundaries and is buffered until then.

use crate::shared;
use frame_support::{pallet_prelude::*, weights::constants::WEIGHT_PER_MILLIS};
use frame_system::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode};
use primitives::v1::{Balance, SessionIndex, MAX_CODE_SIZE, MAX_HEAD_DATA_SIZE, MAX_POV_SIZE};
use sp_runtime::traits::Zero;
use sp_std::prelude::*;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub use pallet::*;

pub mod migration;

const LOG_TARGET: &str = "runtime::configuration";

/// All configuration of the runtime with respect to parachains and parathreads.
#[derive(Clone, Encode, Decode, PartialEq, sp_core::RuntimeDebug, scale_info::TypeInfo)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct HostConfiguration<BlockNumber> {
	// NOTE: This structure is used by parachains via merkle proofs. Therefore, this struct requires
	// special treatment.
	//
	// A parachain requested this struct can only depend on the subset of this struct. Specifically,
	// only a first few fields can be depended upon. These fields cannot be changed without
	// corresponding migration of the parachains.
	/**
	 * The parameters that are required for the parachains.
	 */

	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub max_upward_queue_size: u32,
	/// The maximum size of an upward message that can be sent by a candidate.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_size: u32,
	/// The maximum number of messages that a candidate can contain.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_num_per_candidate: u32,
	/// The maximum number of outbound HRMP messages can be sent by a candidate.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_max_message_num_per_candidate: u32,
	/// The minimum period, in blocks, between which parachains can update their validation code.
	///
	/// If PVF pre-checking is enabled this should be greater than the maximum number of blocks
	/// PVF pre-checking can take. Intuitively, this number should be greater than the duration
	/// specified by [`pvf_voting_ttl`]. Unlike, [`pvf_voting_ttl`], this parameter uses blocks
	/// as a unit.
	pub validation_upgrade_frequency: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,

	/**
	 * The parameters that are not essential, but still may be of interest for parachains.
	 */

	/// The maximum POV block size, in bytes.
	pub max_pov_size: u32,
	/// The maximum size of a message that can be put in a downward message queue.
	///
	/// Since we require receiving at least one DMP message the obvious upper bound of the size is
	/// the PoV size. Of course, there is a lot of other different things that a parachain may
	/// decide to do with its PoV so this value in practice will be picked as a fraction of the PoV
	/// size.
	pub max_downward_message_size: u32,
	/// The amount of weight we wish to devote to the processing the dispatchable upward messages
	/// stage.
	///
	/// NOTE that this is a soft limit and could be exceeded.
	pub ump_service_total_weight: Weight,
	/// The maximum number of outbound HRMP channels a parachain is allowed to open.
	pub hrmp_max_parachain_outbound_channels: u32,
	/// The maximum number of outbound HRMP channels a parathread is allowed to open.
	pub hrmp_max_parathread_outbound_channels: u32,
	/// The deposit that the sender should provide for opening an HRMP channel.
	pub hrmp_sender_deposit: Balance,
	/// The deposit that the recipient should provide for accepting opening an HRMP channel.
	pub hrmp_recipient_deposit: Balance,
	/// The maximum number of messages allowed in an HRMP channel at once.
	pub hrmp_channel_max_capacity: u32,
	/// The maximum total size of messages in bytes allowed in an HRMP channel at once.
	pub hrmp_channel_max_total_size: u32,
	/// The maximum number of inbound HRMP channels a parachain is allowed to accept.
	pub hrmp_max_parachain_inbound_channels: u32,
	/// The maximum number of inbound HRMP channels a parathread is allowed to accept.
	pub hrmp_max_parathread_inbound_channels: u32,
	/// The maximum size of a message that could ever be put into an HRMP channel.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_channel_max_message_size: u32,

	/**
	 * Parameters that will unlikely be needed by parachains.
	 */

	/// How long to keep code on-chain, in blocks. This should be sufficiently long that disputes
	/// have concluded.
	pub code_retention_period: BlockNumber,
	/// The amount of execution cores to dedicate to parathread execution.
	pub parathread_cores: u32,
	/// The number of retries that a parathread author has to submit their block.
	pub parathread_retries: u32,
	/// How often parachain groups should be rotated across parachains.
	///
	/// Must be non-zero.
	pub group_rotation_frequency: BlockNumber,
	/// The availability period, in blocks, for parachains. This is the amount of blocks
	/// after inclusion that validators have to make the block available and signal its availability to
	/// the chain.
	///
	/// Must be at least 1.
	pub chain_availability_period: BlockNumber,
	/// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
	/// but a differing timeout due to differing requirements.
	///
	/// Must be at least 1.
	pub thread_availability_period: BlockNumber,
	/// The amount of blocks ahead to schedule parachains and parathreads.
	pub scheduling_lookahead: u32,
	/// The maximum number of validators to have per core.
	///
	/// `None` means no maximum.
	pub max_validators_per_core: Option<u32>,
	/// The maximum number of validators to use for parachain consensus, period.
	///
	/// `None` means no maximum.
	pub max_validators: Option<u32>,
	/// The amount of sessions to keep for disputes.
	pub dispute_period: SessionIndex,
	/// How long after dispute conclusion to accept statements.
	pub dispute_post_conclusion_acceptance_period: BlockNumber,
	/// The maximum number of dispute spam slots
	pub dispute_max_spam_slots: u32,
	/// How long it takes for a dispute to conclude by time-out, if no supermajority is reached.
	pub dispute_conclusion_by_time_out_period: BlockNumber,
	/// The amount of consensus slots that must pass between submitting an assignment and
	/// submitting an approval vote before a validator is considered a no-show.
	///
	/// Must be at least 1.
	pub no_show_slots: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// The width of the zeroth delay tranche for approval assignments. This many delay tranches
	/// beyond 0 are all consolidated to form a wide 0 tranche.
	pub zeroth_delay_tranche_width: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
	/// The number of samples to do of the `RelayVRFModulo` approval assignment criterion.
	pub relay_vrf_modulo_samples: u32,
	/// The maximum amount of weight any individual upward message may consume. Messages above this
	/// weight go into the overweight queue and may only be serviced explicitly.
	pub ump_max_individual_weight: Weight,
	/// This flag controls whether PVF pre-checking is enabled.
	///
	/// If the flag is false, the behavior should be exactly the same as prior. Specifically, the
	/// upgrade procedure is time-based and parachains that do not look at the go-ahead signal
	/// should still work.
	pub pvf_checking_enabled: bool,
	/// If an active PVF pre-checking vote observes this many number of sessions it gets automatically
	/// rejected.
	///
	/// 0 means PVF pre-checking will be rejected on the first observed session unless the voting
	/// gained supermajority before that the session change.
	pub pvf_voting_ttl: SessionIndex,
	/// The lower bound number of blocks an upgrade can be scheduled.
	///
	/// Typically, upgrade gets scheduled [`validation_upgrade_delay`] relay-chain blocks after
	/// the relay-parent of the parablock that signalled the validation code upgrade. However,
	/// in the case a pre-checking voting was concluded in a longer duration the upgrade will be
	/// scheduled to the next block.
	///
	/// That can disrupt parachain inclusion. Specifically, it will make the blocks that were
	/// already backed invalid.
	///
	/// To prevent that, we introduce the minimum number of blocks after which the upgrade can be
	/// scheduled. This number is controlled by this field.
	pub minimum_validation_upgrade_delay: BlockNumber,
}

impl<BlockNumber: Default + From<u32>> Default for HostConfiguration<BlockNumber> {
	fn default() -> Self {
		Self {
			group_rotation_frequency: 1u32.into(),
			chain_availability_period: 1u32.into(),
			thread_availability_period: 1u32.into(),
			no_show_slots: 1u32.into(),
			validation_upgrade_frequency: Default::default(),
			validation_upgrade_delay: Default::default(),
			code_retention_period: Default::default(),
			max_code_size: Default::default(),
			max_pov_size: Default::default(),
			max_head_data_size: Default::default(),
			parathread_cores: Default::default(),
			parathread_retries: Default::default(),
			scheduling_lookahead: Default::default(),
			max_validators_per_core: Default::default(),
			max_validators: None,
			dispute_period: 6,
			dispute_post_conclusion_acceptance_period: 100.into(),
			dispute_max_spam_slots: 2,
			dispute_conclusion_by_time_out_period: 200.into(),
			n_delay_tranches: Default::default(),
			zeroth_delay_tranche_width: Default::default(),
			needed_approvals: Default::default(),
			relay_vrf_modulo_samples: Default::default(),
			max_upward_queue_count: Default::default(),
			max_upward_queue_size: Default::default(),
			max_downward_message_size: Default::default(),
			ump_service_total_weight: Default::default(),
			max_upward_message_size: Default::default(),
			max_upward_message_num_per_candidate: Default::default(),
			hrmp_sender_deposit: Default::default(),
			hrmp_recipient_deposit: Default::default(),
			hrmp_channel_max_capacity: Default::default(),
			hrmp_channel_max_total_size: Default::default(),
			hrmp_max_parachain_inbound_channels: Default::default(),
			hrmp_max_parathread_inbound_channels: Default::default(),
			hrmp_channel_max_message_size: Default::default(),
			hrmp_max_parachain_outbound_channels: Default::default(),
			hrmp_max_parathread_outbound_channels: Default::default(),
			hrmp_max_message_num_per_candidate: Default::default(),
			ump_max_individual_weight: 20 * WEIGHT_PER_MILLIS,
			pvf_checking_enabled: false,
			pvf_voting_ttl: 2u32.into(),
			minimum_validation_upgrade_delay: 0.into(),
		}
	}
}

impl<BlockNumber: Zero> HostConfiguration<BlockNumber> {
	/// Checks that this instance is consistent with the requirements on each individual member.
	///
	/// # Panic
	///
	/// This function panics if any member is not set properly.
	pub fn check_consistency(&self) {
		if self.group_rotation_frequency.is_zero() {
			panic!("`group_rotation_frequency` must be non-zero!")
		}

		if self.chain_availability_period.is_zero() {
			panic!("`chain_availability_period` must be at least 1!")
		}

		if self.thread_availability_period.is_zero() {
			panic!("`thread_availability_period` must be at least 1!")
		}

		if self.no_show_slots.is_zero() {
			panic!("`no_show_slots` must be at least 1!")
		}

		if self.max_code_size > MAX_CODE_SIZE {
			panic!(
				"`max_code_size` ({}) is bigger than allowed by the client ({})",
				self.max_code_size, MAX_CODE_SIZE,
			)
		}

		if self.max_head_data_size > MAX_HEAD_DATA_SIZE {
			panic!(
				"`max_head_data_size` ({}) is bigger than allowed by the client ({})",
				self.max_head_data_size, MAX_HEAD_DATA_SIZE
			)
		}

		if self.max_pov_size > MAX_POV_SIZE {
			panic!("`max_pov_size` is bigger than allowed by the client")
		}
	}
}

pub trait WeightInfo {
	fn set_config_with_block_number() -> Weight;
	fn set_config_with_u32() -> Weight;
	fn set_config_with_option_u32() -> Weight;
	fn set_config_with_weight() -> Weight;
	fn set_config_with_balance() -> Weight;
	fn set_hrmp_open_request_ttl() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn set_config_with_block_number() -> Weight {
		Weight::MAX
	}
	fn set_config_with_u32() -> Weight {
		Weight::MAX
	}
	fn set_config_with_option_u32() -> Weight {
		Weight::MAX
	}
	fn set_config_with_weight() -> Weight {
		Weight::MAX
	}
	fn set_config_with_balance() -> Weight {
		Weight::MAX
	}
	fn set_hrmp_open_request_ttl() -> Weight {
		Weight::MAX
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(migration::STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + shared::Config {
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The new value for a configuration parameter is invalid.
		InvalidNewValue,
	}

	/// The active configuration for the current session.
	#[pallet::storage]
	#[pallet::getter(fn config)]
	pub(crate) type ActiveConfig<T: Config> =
		StorageValue<_, HostConfiguration<T::BlockNumber>, ValueQuery>;

	/// Pending configuration (if any) for the next session.
	///
	/// DEPRECATED: This is no longer used, and will be removed in the future.
	#[pallet::storage]
	pub(crate) type PendingConfig<T: Config> =
		StorageMap<_, Twox64Concat, SessionIndex, migration::v1::HostConfiguration<T::BlockNumber>>;

	/// Pending configuration changes.
	///
	/// This is a list of configuration changes, each with a session index at which it should
	/// be applied.
	///
	/// The list is sorted ascending by session index. Also, this list can only contain at most
	/// 2 items: for the next session and for the `scheduled_session`.
	#[pallet::storage]
	pub(crate) type PendingConfigs<T: Config> =
		StorageValue<_, Vec<(SessionIndex, HostConfiguration<T::BlockNumber>)>, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub config: HostConfiguration<T::BlockNumber>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig { config: Default::default() }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			self.config.check_consistency();
			ActiveConfig::<T>::put(&self.config);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set the validation upgrade frequency.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_validation_upgrade_frequency(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.validation_upgrade_frequency = new;
			});
			Ok(())
		}

		/// Set the validation upgrade delay.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_validation_upgrade_delay(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.validation_upgrade_delay = new;
			});
			Ok(())
		}

		/// Set the acceptance period for an included candidate.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_code_retention_period(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.code_retention_period = new;
			});
			Ok(())
		}

		/// Set the max validation code size for incoming upgrades.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_code_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(new <= MAX_CODE_SIZE, Error::<T>::InvalidNewValue);
			Self::update_config_member(|config| {
				config.max_code_size = new;
			});
			Ok(())
		}

		/// Set the max POV block size for incoming upgrades.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_pov_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(new <= MAX_POV_SIZE, Error::<T>::InvalidNewValue);
			Self::update_config_member(|config| {
				config.max_pov_size = new;
			});
			Ok(())
		}

		/// Set the max head data size for paras.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_head_data_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(new <= MAX_HEAD_DATA_SIZE, Error::<T>::InvalidNewValue);
			Self::update_config_member(|config| {
				config.max_head_data_size = new;
			});
			Ok(())
		}

		/// Set the number of parathread execution cores.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_parathread_cores(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.parathread_cores = new;
			});
			Ok(())
		}

		/// Set the number of retries for a particular parathread.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_parathread_retries(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.parathread_retries = new;
			});
			Ok(())
		}

		/// Set the parachain validator-group rotation frequency
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_group_rotation_frequency(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;

			ensure!(!new.is_zero(), Error::<T>::InvalidNewValue);

			Self::update_config_member(|config| {
				config.group_rotation_frequency = new;
			});
			Ok(())
		}

		/// Set the availability period for parachains.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_chain_availability_period(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;

			ensure!(!new.is_zero(), Error::<T>::InvalidNewValue);

			Self::update_config_member(|config| {
				config.chain_availability_period = new;
			});
			Ok(())
		}

		/// Set the availability period for parathreads.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_thread_availability_period(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;

			ensure!(!new.is_zero(), Error::<T>::InvalidNewValue);

			Self::update_config_member(|config| {
				config.thread_availability_period = new;
			});
			Ok(())
		}

		/// Set the scheduling lookahead, in expected number of blocks at peak throughput.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_scheduling_lookahead(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.scheduling_lookahead = new;
			});
			Ok(())
		}

		/// Set the maximum number of validators to assign to any core.
		#[pallet::weight((
			T::WeightInfo::set_config_with_option_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_validators_per_core(
			origin: OriginFor<T>,
			new: Option<u32>,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_validators_per_core = new;
			});
			Ok(())
		}

		/// Set the maximum number of validators to use in parachain consensus.
		#[pallet::weight((
			T::WeightInfo::set_config_with_option_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_validators(origin: OriginFor<T>, new: Option<u32>) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_validators = new;
			});
			Ok(())
		}

		/// Set the dispute period, in number of sessions to keep for disputes.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_dispute_period(origin: OriginFor<T>, new: SessionIndex) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.dispute_period = new;
			});
			Ok(())
		}

		/// Set the dispute post conclusion acceptance period.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_dispute_post_conclusion_acceptance_period(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.dispute_post_conclusion_acceptance_period = new;
			});
			Ok(())
		}

		/// Set the maximum number of dispute spam slots.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_dispute_max_spam_slots(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.dispute_max_spam_slots = new;
			});
			Ok(())
		}

		/// Set the dispute conclusion by time out period.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_dispute_conclusion_by_time_out_period(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.dispute_conclusion_by_time_out_period = new;
			});
			Ok(())
		}

		/// Set the no show slots, in number of number of consensus slots.
		/// Must be at least 1.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_no_show_slots(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;

			ensure!(!new.is_zero(), Error::<T>::InvalidNewValue);

			Self::update_config_member(|config| {
				config.no_show_slots = new;
			});
			Ok(())
		}

		/// Set the total number of delay tranches.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_n_delay_tranches(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.n_delay_tranches = new;
			});
			Ok(())
		}

		/// Set the zeroth delay tranche width.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_zeroth_delay_tranche_width(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.zeroth_delay_tranche_width = new;
			});
			Ok(())
		}

		/// Set the number of validators needed to approve a block.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_needed_approvals(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.needed_approvals = new;
			});
			Ok(())
		}

		/// Set the number of samples to do of the `RelayVRFModulo` approval assignment criterion.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_relay_vrf_modulo_samples(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.relay_vrf_modulo_samples = new;
			});
			Ok(())
		}

		/// Sets the maximum items that can present in a upward dispatch queue at once.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_upward_queue_count(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_upward_queue_count = new;
			});
			Ok(())
		}

		/// Sets the maximum total size of items that can present in a upward dispatch queue at once.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_upward_queue_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_upward_queue_size = new;
			});
			Ok(())
		}

		/// Set the critical downward message size.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_downward_message_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_downward_message_size = new;
			});
			Ok(())
		}

		/// Sets the soft limit for the phase of dispatching dispatchable upward messages.
		#[pallet::weight((
			T::WeightInfo::set_config_with_weight(),
			DispatchClass::Operational,
		))]
		pub fn set_ump_service_total_weight(origin: OriginFor<T>, new: Weight) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.ump_service_total_weight = new;
			});
			Ok(())
		}

		/// Sets the maximum size of an upward message that can be sent by a candidate.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_upward_message_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_upward_message_size = new;
			});
			Ok(())
		}

		/// Sets the maximum number of messages that a candidate can contain.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_max_upward_message_num_per_candidate(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.max_upward_message_num_per_candidate = new;
			});
			Ok(())
		}

		/// Sets the number of sessions after which an HRMP open channel request expires.
		#[pallet::weight((
			T::WeightInfo::set_hrmp_open_request_ttl(),
			DispatchClass::Operational,
		))]
		// Deprecated, but is not marked as such, because that would trigger warnings coming from
		// the macro.
		pub fn set_hrmp_open_request_ttl(_origin: OriginFor<T>, _new: u32) -> DispatchResult {
			Err("this doesn't have any effect".into())
		}

		/// Sets the amount of funds that the sender should provide for opening an HRMP channel.
		#[pallet::weight((
			T::WeightInfo::set_config_with_balance(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_sender_deposit(origin: OriginFor<T>, new: Balance) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_sender_deposit = new;
			});
			Ok(())
		}

		/// Sets the amount of funds that the recipient should provide for accepting opening an HRMP
		/// channel.
		#[pallet::weight((
			T::WeightInfo::set_config_with_balance(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_recipient_deposit(origin: OriginFor<T>, new: Balance) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_recipient_deposit = new;
			});
			Ok(())
		}

		/// Sets the maximum number of messages allowed in an HRMP channel at once.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_channel_max_capacity(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_channel_max_capacity = new;
			});
			Ok(())
		}

		/// Sets the maximum total size of messages in bytes allowed in an HRMP channel at once.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_channel_max_total_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_channel_max_total_size = new;
			});
			Ok(())
		}

		/// Sets the maximum number of inbound HRMP channels a parachain is allowed to accept.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_max_parachain_inbound_channels(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_max_parachain_inbound_channels = new;
			});
			Ok(())
		}

		/// Sets the maximum number of inbound HRMP channels a parathread is allowed to accept.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_max_parathread_inbound_channels(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_max_parathread_inbound_channels = new;
			});
			Ok(())
		}

		/// Sets the maximum size of a message that could ever be put into an HRMP channel.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_channel_max_message_size(origin: OriginFor<T>, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_channel_max_message_size = new;
			});
			Ok(())
		}

		/// Sets the maximum number of outbound HRMP channels a parachain is allowed to open.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_max_parachain_outbound_channels(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_max_parachain_outbound_channels = new;
			});
			Ok(())
		}

		/// Sets the maximum number of outbound HRMP channels a parathread is allowed to open.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_max_parathread_outbound_channels(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_max_parathread_outbound_channels = new;
			});
			Ok(())
		}

		/// Sets the maximum number of outbound HRMP messages can be sent by a candidate.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_hrmp_max_message_num_per_candidate(
			origin: OriginFor<T>,
			new: u32,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.hrmp_max_message_num_per_candidate = new;
			});
			Ok(())
		}

		/// Sets the maximum amount of weight any individual upward message may consume.
		#[pallet::weight((
			T::WeightInfo::set_config_with_weight(),
			DispatchClass::Operational,
		))]
		pub fn set_ump_max_individual_weight(origin: OriginFor<T>, new: Weight) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.ump_max_individual_weight = new;
			});
			Ok(())
		}

		/// Enable or disable PVF pre-checking. Consult the field documentation prior executing.
		#[pallet::weight((
			// Using u32 here is a little bit of cheating, but that should be fine.
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_pvf_checking_enabled(origin: OriginFor<T>, new: bool) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.pvf_checking_enabled = new;
			});
			Ok(())
		}

		/// Set the number of session changes after which a PVF pre-checking voting is rejected.
		#[pallet::weight((
			T::WeightInfo::set_config_with_u32(),
			DispatchClass::Operational,
		))]
		pub fn set_pvf_voting_ttl(origin: OriginFor<T>, new: SessionIndex) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.pvf_voting_ttl = new;
			});
			Ok(())
		}

		/// Sets the minimum delay between announcing the upgrade block for a parachain until the
		/// upgrade taking place.
		#[pallet::weight((
			T::WeightInfo::set_config_with_block_number(),
			DispatchClass::Operational,
		))]
		pub fn set_minimum_validation_upgrade_delay(
			origin: OriginFor<T>,
			new: T::BlockNumber,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				config.minimum_validation_upgrade_delay = new;
			});
			Ok(())
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_runtime_upgrade() -> Weight {
			migration::migrate_to_latest::<T>()
		}

		fn integrity_test() {
			assert_eq!(
				&ActiveConfig::<T>::hashed_key(),
				primitives::v1::well_known_keys::ACTIVE_CONFIG,
				"`well_known_keys::ACTIVE_CONFIG` doesn't match key of `ActiveConfig`! Make sure that the name of the\
				 configuration pallet is `Configuration` in the runtime!",
			);
		}
	}
}

/// A struct that holds the configuration that was active before the session change and optionally
/// a configuration that became active after the session change.
pub struct SessionChangeOutcome<BlockNumber> {
	/// Previously active configuration.
	pub prev_config: HostConfiguration<BlockNumber>,
	/// If new configuration was applied during the session change, this is the new configuration.
	pub new_config: Option<HostConfiguration<BlockNumber>>,
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	///
	/// Returns the configuration that was actual before the session change and the configuration
	/// that became active after the session change. If there were no scheduled changes, both will
	/// be the same.
	pub(crate) fn initializer_on_new_session(
		session_index: &SessionIndex,
	) -> SessionChangeOutcome<T::BlockNumber> {
		let pending_configs = <PendingConfigs<T>>::get();
		let prev_config = <Self as Store>::ActiveConfig::get();

		// No pending configuration changes, so we're done.
		if pending_configs.is_empty() {
			return SessionChangeOutcome { prev_config, new_config: None }
		}

		let (mut past_and_present, future) = pending_configs
			.into_iter()
			.partition::<Vec<_>, _>(|&(apply_at_session, _)| apply_at_session <= *session_index);

		if past_and_present.len() > 1 {
			// This should never happen since we schedule configuration changes only into the future
			// sessions and this handler called for each session change.
			log::error!(
				target: LOG_TARGET,
				"Skipping applying configuration changes scheduled sessions in the past",
			);
		}

		let new_config = past_and_present.pop().map(|(_, config)| config);
		if let Some(ref new_config) = new_config {
			// Apply the new configuration.
			<Self as Store>::ActiveConfig::put(new_config);
		}

		<PendingConfigs<T>>::put(future);

		SessionChangeOutcome { prev_config, new_config }
	}

	/// Return the session index that should be used for any future scheduled changes.
	fn scheduled_session() -> SessionIndex {
		shared::Pallet::<T>::scheduled_session()
	}

	/// Forcibly set the active config. This should be used with extreme care, and typically
	/// only when enabling parachains runtime modules for the first time on a chain which has
	/// been running without them.
	pub fn force_set_active_config(config: HostConfiguration<T::BlockNumber>) {
		<Self as Store>::ActiveConfig::set(config);
	}

	// NOTE: Explicitly tell rustc not to inline this because otherwise heuristics note the incoming
	// closure making it's attractive to inline. However, in this case, we will end up with lots of
	// duplicated code (making this function to show up in the top of heaviest functions) only for
	// the sake of essentially avoiding an indirect call. Doesn't worth it.
	#[inline(never)]
	fn update_config_member(updater: impl FnOnce(&mut HostConfiguration<T::BlockNumber>)) {
		let mut pending_configs = <PendingConfigs<T>>::get();

		// 1. pending_configs = []
		//    No pending configuration changes.
		//
		//    That means we should use the active config as the base configuration. We will insert
		//    the new pending configuration as (cur+2, new_config) into the list.
		//
		// 2. pending_configs = [(cur+2, X)]
		//    There is a configuration that is pending for the scheduled session.
		//
		//    We will use X as the base configuration. We can update the pending configuration X
		//    directly.
		//
		// 3. pending_configs = [(cur+1, X)]
		//    There is a pending configuration scheduled and it will be applied in the next session.
		//
		//    We will use X as the base configuration. We need to schedule a new configuration change
		//    for the `scheduled_session` and use X as the base for the new configuration.
		//
		// 4. pending_configs = [(cur+1, X), (cur+2, Y)]
		//    There is a pending configuration change in the next session and for the scheduled
		//    session. Due to case №3, we can be sure that Y is based on top of X. This means we
		//    can use Y as the base configuration and update Y directly.
		//
		// There cannot be (cur, X) because those are applied in the session change handler for the
		// current session.

		// First, we need to decide what we should use as the base configuration.
		let mut base_config = pending_configs
			.last()
			.map(|&(_, ref config)| config.clone())
			.unwrap_or_else(Self::config);

		// Now, we need to decide what the new configuration should be.
		updater(&mut base_config);

		let scheduled_session = Self::scheduled_session();

		if let Some(&mut (_, ref mut config)) = pending_configs
			.iter_mut()
			.find(|&&mut (apply_at_session, _)| apply_at_session >= scheduled_session)
		{
			*config = base_config;
		} else {
			// We are scheduling a new configuration change for the scheduled session.
			pending_configs.push((scheduled_session, base_config));
		}

		<PendingConfigs<T>>::put(pending_configs);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Configuration, Origin, ParasShared};

	use frame_support::assert_ok;

	fn on_new_session(
		session_index: SessionIndex,
	) -> (HostConfiguration<u32>, HostConfiguration<u32>) {
		ParasShared::set_session_index(session_index);
		let SessionChangeOutcome { prev_config, new_config } =
			Configuration::initializer_on_new_session(&session_index);
		let new_config = new_config.unwrap_or_else(|| prev_config.clone());
		(prev_config, new_config)
	}

	#[test]
	fn scheduled_session_is_two_sessions_from_now() {
		new_test_ext(Default::default()).execute_with(|| {
			// The logic here is really tested only with scheduled_session = 2. It should work
			// with other values, but that should receive a more rigorious testing.
			on_new_session(1);
			assert_eq!(Configuration::scheduled_session(), 3);
		});
	}

	#[test]
	fn initializer_on_new_session() {
		new_test_ext(Default::default()).execute_with(|| {
			let (prev_config, new_config) = on_new_session(1);
			assert_eq!(prev_config, new_config);
			assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));

			let (prev_config, new_config) = on_new_session(2);
			assert_eq!(prev_config, new_config);

			let (prev_config, new_config) = on_new_session(3);
			assert_eq!(prev_config, HostConfiguration::default());
			assert_eq!(
				new_config,
				HostConfiguration { validation_upgrade_delay: 100, ..prev_config }
			);
		});
	}

	#[test]
	fn config_changes_after_2_session_boundary() {
		new_test_ext(Default::default()).execute_with(|| {
			let old_config = Configuration::config();
			let mut config = old_config.clone();
			config.validation_upgrade_delay = 100;
			assert!(old_config != config);

			assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));

			// Verify that the current configuration has not changed and that there is a scheduled
			// change for the SESSION_DELAY sessions in advance.
			assert_eq!(Configuration::config(), old_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![(2, config.clone())]);

			on_new_session(1);

			// One session has passed, we should be still waiting for the pending configuration.
			assert_eq!(Configuration::config(), old_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![(2, config.clone())]);

			on_new_session(2);

			assert_eq!(Configuration::config(), config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![]);
		})
	}

	#[test]
	fn consecutive_changes_within_one_session() {
		new_test_ext(Default::default()).execute_with(|| {
			let old_config = Configuration::config();
			let mut config = old_config.clone();
			config.validation_upgrade_delay = 100;
			config.validation_upgrade_frequency = 100;
			assert!(old_config != config);

			assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));
			assert_ok!(Configuration::set_validation_upgrade_frequency(Origin::root(), 100));
			assert_eq!(Configuration::config(), old_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![(2, config.clone())]);

			on_new_session(1);

			assert_eq!(Configuration::config(), old_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![(2, config.clone())]);

			on_new_session(2);

			assert_eq!(Configuration::config(), config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![]);
		});
	}

	#[test]
	fn pending_next_session_but_we_upgrade_once_more() {
		new_test_ext(Default::default()).execute_with(|| {
			let initial_config = Configuration::config();
			let intermediate_config =
				HostConfiguration { validation_upgrade_delay: 100, ..initial_config.clone() };
			let final_config = HostConfiguration {
				validation_upgrade_delay: 100,
				validation_upgrade_frequency: 99,
				..initial_config.clone()
			};

			assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));
			assert_eq!(Configuration::config(), initial_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(2, intermediate_config.clone())]
			);

			on_new_session(1);

			// We are still waiting until the pending configuration is applied and we add another
			// update.
			assert_ok!(Configuration::set_validation_upgrade_frequency(Origin::root(), 99));

			// This should result in yet another configiguration change scheduled.
			assert_eq!(Configuration::config(), initial_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(2, intermediate_config.clone()), (3, final_config.clone())]
			);

			on_new_session(2);

			assert_eq!(Configuration::config(), intermediate_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(3, final_config.clone())]
			);

			on_new_session(3);

			assert_eq!(Configuration::config(), final_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![]);
		});
	}

	#[test]
	fn scheduled_session_config_update_while_next_session_pending() {
		new_test_ext(Default::default()).execute_with(|| {
			let initial_config = Configuration::config();
			let intermediate_config =
				HostConfiguration { validation_upgrade_delay: 100, ..initial_config.clone() };
			let final_config = HostConfiguration {
				validation_upgrade_delay: 100,
				validation_upgrade_frequency: 99,
				code_retention_period: 98,
				..initial_config.clone()
			};

			assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));
			assert_eq!(Configuration::config(), initial_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(2, intermediate_config.clone())]
			);

			on_new_session(1);

			// The second call should fall into the case where we already have a pending config
			// update for the scheduled_session, but we want to update it once more.
			assert_ok!(Configuration::set_validation_upgrade_frequency(Origin::root(), 99));
			assert_ok!(Configuration::set_code_retention_period(Origin::root(), 98));

			// This should result in yet another configiguration change scheduled.
			assert_eq!(Configuration::config(), initial_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(2, intermediate_config.clone()), (3, final_config.clone())]
			);

			on_new_session(2);

			assert_eq!(Configuration::config(), intermediate_config);
			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(3, final_config.clone())]
			);

			on_new_session(3);

			assert_eq!(Configuration::config(), final_config);
			assert_eq!(<Configuration as Store>::PendingConfigs::get(), vec![]);
		});
	}

	#[test]
	fn setting_pending_config_members() {
		new_test_ext(Default::default()).execute_with(|| {
			let new_config = HostConfiguration {
				validation_upgrade_frequency: 100,
				validation_upgrade_delay: 10,
				code_retention_period: 5,
				max_code_size: 100_000,
				max_pov_size: 1024,
				max_head_data_size: 1_000,
				parathread_cores: 2,
				parathread_retries: 5,
				group_rotation_frequency: 20,
				chain_availability_period: 10,
				thread_availability_period: 8,
				scheduling_lookahead: 3,
				max_validators_per_core: None,
				max_validators: None,
				dispute_period: 239,
				dispute_post_conclusion_acceptance_period: 10,
				dispute_max_spam_slots: 2,
				dispute_conclusion_by_time_out_period: 512,
				no_show_slots: 240,
				n_delay_tranches: 241,
				zeroth_delay_tranche_width: 242,
				needed_approvals: 242,
				relay_vrf_modulo_samples: 243,
				max_upward_queue_count: 1337,
				max_upward_queue_size: 228,
				max_downward_message_size: 2048,
				ump_service_total_weight: 20000,
				max_upward_message_size: 448,
				max_upward_message_num_per_candidate: 5,
				hrmp_sender_deposit: 22,
				hrmp_recipient_deposit: 4905,
				hrmp_channel_max_capacity: 3921,
				hrmp_channel_max_total_size: 7687,
				hrmp_max_parachain_inbound_channels: 3722,
				hrmp_max_parathread_inbound_channels: 1967,
				hrmp_channel_max_message_size: 8192,
				hrmp_max_parachain_outbound_channels: 100,
				hrmp_max_parathread_outbound_channels: 200,
				hrmp_max_message_num_per_candidate: 20,
				ump_max_individual_weight: 909,
				pvf_checking_enabled: true,
				pvf_voting_ttl: 3,
				minimum_validation_upgrade_delay: 20,
			};

			assert!(<Configuration as Store>::PendingConfig::get(shared::SESSION_DELAY).is_none());

			Configuration::set_validation_upgrade_frequency(
				Origin::root(),
				new_config.validation_upgrade_frequency,
			)
			.unwrap();
			Configuration::set_validation_upgrade_delay(
				Origin::root(),
				new_config.validation_upgrade_delay,
			)
			.unwrap();
			Configuration::set_code_retention_period(
				Origin::root(),
				new_config.code_retention_period,
			)
			.unwrap();
			Configuration::set_max_code_size(Origin::root(), new_config.max_code_size).unwrap();
			Configuration::set_max_pov_size(Origin::root(), new_config.max_pov_size).unwrap();
			Configuration::set_max_head_data_size(Origin::root(), new_config.max_head_data_size)
				.unwrap();
			Configuration::set_parathread_cores(Origin::root(), new_config.parathread_cores)
				.unwrap();
			Configuration::set_parathread_retries(Origin::root(), new_config.parathread_retries)
				.unwrap();
			Configuration::set_group_rotation_frequency(
				Origin::root(),
				new_config.group_rotation_frequency,
			)
			.unwrap();
			Configuration::set_chain_availability_period(
				Origin::root(),
				new_config.chain_availability_period,
			)
			.unwrap();
			Configuration::set_thread_availability_period(
				Origin::root(),
				new_config.thread_availability_period,
			)
			.unwrap();
			Configuration::set_scheduling_lookahead(
				Origin::root(),
				new_config.scheduling_lookahead,
			)
			.unwrap();
			Configuration::set_max_validators_per_core(
				Origin::root(),
				new_config.max_validators_per_core,
			)
			.unwrap();
			Configuration::set_max_validators(Origin::root(), new_config.max_validators).unwrap();
			Configuration::set_dispute_period(Origin::root(), new_config.dispute_period).unwrap();
			Configuration::set_dispute_post_conclusion_acceptance_period(
				Origin::root(),
				new_config.dispute_post_conclusion_acceptance_period,
			)
			.unwrap();
			Configuration::set_dispute_max_spam_slots(
				Origin::root(),
				new_config.dispute_max_spam_slots,
			)
			.unwrap();
			Configuration::set_dispute_conclusion_by_time_out_period(
				Origin::root(),
				new_config.dispute_conclusion_by_time_out_period,
			)
			.unwrap();
			Configuration::set_no_show_slots(Origin::root(), new_config.no_show_slots).unwrap();
			Configuration::set_n_delay_tranches(Origin::root(), new_config.n_delay_tranches)
				.unwrap();
			Configuration::set_zeroth_delay_tranche_width(
				Origin::root(),
				new_config.zeroth_delay_tranche_width,
			)
			.unwrap();
			Configuration::set_needed_approvals(Origin::root(), new_config.needed_approvals)
				.unwrap();
			Configuration::set_relay_vrf_modulo_samples(
				Origin::root(),
				new_config.relay_vrf_modulo_samples,
			)
			.unwrap();
			Configuration::set_max_upward_queue_count(
				Origin::root(),
				new_config.max_upward_queue_count,
			)
			.unwrap();
			Configuration::set_max_upward_queue_size(
				Origin::root(),
				new_config.max_upward_queue_size,
			)
			.unwrap();
			Configuration::set_max_downward_message_size(
				Origin::root(),
				new_config.max_downward_message_size,
			)
			.unwrap();
			Configuration::set_ump_service_total_weight(
				Origin::root(),
				new_config.ump_service_total_weight,
			)
			.unwrap();
			Configuration::set_max_upward_message_size(
				Origin::root(),
				new_config.max_upward_message_size,
			)
			.unwrap();
			Configuration::set_max_upward_message_num_per_candidate(
				Origin::root(),
				new_config.max_upward_message_num_per_candidate,
			)
			.unwrap();
			Configuration::set_hrmp_sender_deposit(Origin::root(), new_config.hrmp_sender_deposit)
				.unwrap();
			Configuration::set_hrmp_recipient_deposit(
				Origin::root(),
				new_config.hrmp_recipient_deposit,
			)
			.unwrap();
			Configuration::set_hrmp_channel_max_capacity(
				Origin::root(),
				new_config.hrmp_channel_max_capacity,
			)
			.unwrap();
			Configuration::set_hrmp_channel_max_total_size(
				Origin::root(),
				new_config.hrmp_channel_max_total_size,
			)
			.unwrap();
			Configuration::set_hrmp_max_parachain_inbound_channels(
				Origin::root(),
				new_config.hrmp_max_parachain_inbound_channels,
			)
			.unwrap();
			Configuration::set_hrmp_max_parathread_inbound_channels(
				Origin::root(),
				new_config.hrmp_max_parathread_inbound_channels,
			)
			.unwrap();
			Configuration::set_hrmp_channel_max_message_size(
				Origin::root(),
				new_config.hrmp_channel_max_message_size,
			)
			.unwrap();
			Configuration::set_hrmp_max_parachain_outbound_channels(
				Origin::root(),
				new_config.hrmp_max_parachain_outbound_channels,
			)
			.unwrap();
			Configuration::set_hrmp_max_parathread_outbound_channels(
				Origin::root(),
				new_config.hrmp_max_parathread_outbound_channels,
			)
			.unwrap();
			Configuration::set_hrmp_max_message_num_per_candidate(
				Origin::root(),
				new_config.hrmp_max_message_num_per_candidate,
			)
			.unwrap();
			Configuration::set_ump_max_individual_weight(
				Origin::root(),
				new_config.ump_max_individual_weight,
			)
			.unwrap();
			Configuration::set_pvf_checking_enabled(
				Origin::root(),
				new_config.pvf_checking_enabled,
			)
			.unwrap();
			Configuration::set_pvf_voting_ttl(Origin::root(), new_config.pvf_voting_ttl).unwrap();
			Configuration::set_minimum_validation_upgrade_delay(
				Origin::root(),
				new_config.minimum_validation_upgrade_delay,
			)
			.unwrap();

			assert_eq!(
				<Configuration as Store>::PendingConfigs::get(),
				vec![(shared::SESSION_DELAY, new_config)],
			);
		})
	}

	#[test]
	fn non_root_cannot_set_config() {
		new_test_ext(Default::default()).execute_with(|| {
			assert!(Configuration::set_validation_upgrade_delay(Origin::signed(1), 100).is_err());
		});
	}

	#[test]
	fn verify_externally_accessible() {
		// This test verifies that the value can be accessed through the well known keys and the
		// host configuration decodes into the abridged version.

		use primitives::v1::{well_known_keys, AbridgedHostConfiguration};

		new_test_ext(Default::default()).execute_with(|| {
			let ground_truth = HostConfiguration::default();

			// Make sure that the configuration is stored in the storage.
			<Configuration as Store>::ActiveConfig::put(ground_truth.clone());

			// Extract the active config via the well known key.
			let raw_active_config = sp_io::storage::get(well_known_keys::ACTIVE_CONFIG)
				.expect("config must be present in storage under ACTIVE_CONFIG");
			let abridged_config = AbridgedHostConfiguration::decode(&mut &raw_active_config[..])
				.expect("HostConfiguration must be decodable into AbridgedHostConfiguration");

			assert_eq!(
				abridged_config,
				AbridgedHostConfiguration {
					max_code_size: ground_truth.max_code_size,
					max_head_data_size: ground_truth.max_head_data_size,
					max_upward_queue_count: ground_truth.max_upward_queue_count,
					max_upward_queue_size: ground_truth.max_upward_queue_size,
					max_upward_message_size: ground_truth.max_upward_message_size,
					max_upward_message_num_per_candidate: ground_truth
						.max_upward_message_num_per_candidate,
					hrmp_max_message_num_per_candidate: ground_truth
						.hrmp_max_message_num_per_candidate,
					validation_upgrade_frequency: ground_truth.validation_upgrade_frequency,
					validation_upgrade_delay: ground_truth.validation_upgrade_delay,
				},
			);
		});
	}
}
