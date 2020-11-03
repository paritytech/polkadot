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

use sp_std::prelude::*;
use primitives::v1::ValidatorId;
use frame_support::{
	decl_storage, decl_module, decl_error,
	dispatch::DispatchResult,
	weights::{DispatchClass, Weight},
};
use codec::{Encode, Decode};
use frame_system::ensure_root;

/// All configuration of the runtime with respect to parachains and parathreads.
#[derive(Clone, Encode, Decode, PartialEq, Default, sp_core::RuntimeDebug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct HostConfiguration<BlockNumber> {
	/// The minimum frequency at which parachains can update their validation code.
	pub validation_upgrade_frequency: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
	/// The acceptance period, in blocks. This is the amount of blocks after availability that validators
	/// and fishermen have to perform secondary checks or issue reports.
	pub acceptance_period: BlockNumber,
	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// The amount of execution cores to dedicate to parathread execution.
	pub parathread_cores: u32,
	/// The number of retries that a parathread author has to submit their block.
	pub parathread_retries: u32,
	/// How often parachain groups should be rotated across parachains. Must be non-zero.
	pub group_rotation_frequency: BlockNumber,
	/// The availability period, in blocks, for parachains. This is the amount of blocks
	/// after inclusion that validators have to make the block available and signal its availability to
	/// the chain. Must be at least 1.
	pub chain_availability_period: BlockNumber,
	/// The availability period, in blocks, for parathreads. Same as the `chain_availability_period`,
	/// but a differing timeout due to differing requirements. Must be at least 1.
	pub thread_availability_period: BlockNumber,
	/// The amount of blocks ahead to schedule parachains and parathreads.
	pub scheduling_lookahead: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub max_upward_queue_size: u32,
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
	pub preferred_dispatchable_upward_messages_step_weight: Weight,
	/// The maximum size of an upward message that can be sent by a candidate.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_size: u32,
	/// The maximum number of messages that a candidate can contain.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_num_per_candidate: u32,
}

pub trait Trait: frame_system::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Configuration {
		/// The active configuration for the current session.
		Config get(fn config) config(): HostConfiguration<T::BlockNumber>;
		/// Pending configuration (if any) for the next session.
		PendingConfig: Option<HostConfiguration<T::BlockNumber>>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;

		/// Set the validation upgrade frequency.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_validation_upgrade_frequency(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.validation_upgrade_frequency, new) != new
			});
			Ok(())
		}

		/// Set the validation upgrade delay.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_validation_upgrade_delay(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.validation_upgrade_delay, new) != new
			});
			Ok(())
		}

		/// Set the acceptance period for an included candidate.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_acceptance_period(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.acceptance_period, new) != new
			});
			Ok(())
		}

		/// Set the max validation code size for incoming upgrades.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_code_size(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_code_size, new) != new
			});
			Ok(())
		}

		/// Set the max head data size for paras.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_head_data_size(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_head_data_size, new) != new
			});
			Ok(())
		}

		/// Set the number of parathread execution cores.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_parathread_cores(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.parathread_cores, new) != new
			});
			Ok(())
		}

		/// Set the number of retries for a particular parathread.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_parathread_retries(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.parathread_retries, new) != new
			});
			Ok(())
		}


		/// Set the parachain validator-group rotation frequency
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_group_rotation_frequency(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.group_rotation_frequency, new) != new
			});
			Ok(())
		}

		/// Set the availability period for parachains.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_chain_availability_period(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.chain_availability_period, new) != new
			});
			Ok(())
		}

		/// Set the availability period for parathreads.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_thread_availability_period(origin, new: T::BlockNumber) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.thread_availability_period, new) != new
			});
			Ok(())
		}

		/// Set the scheduling lookahead, in expected number of blocks at peak throughput.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_scheduling_lookahead(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.scheduling_lookahead, new) != new
			});
			Ok(())
		}

		/// Sets the maximum items that can present in a upward dispatch queue at once.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_upward_queue_count(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_upward_queue_count, new) != new
			});
			Ok(())
		}

		/// Sets the maximum total size of items that can present in a upward dispatch queue at once.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_upward_queue_size(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_upward_queue_size, new) != new
			});
			Ok(())
		}

		/// Set the critical downward message size.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_downward_message_size(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_downward_message_size, new) != new
			});
			Ok(())
		}

		/// Sets the soft limit for the phase of dispatching dispatchable upward messages.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_preferred_dispatchable_upward_messages_step_weight(origin, new: Weight) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.preferred_dispatchable_upward_messages_step_weight, new) != new
			});
			Ok(())
		}

		/// Sets the maximum size of an upward message that can be sent by a candidate.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_upward_message_size(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_upward_message_size, new) != new
			});
			Ok(())
		}

		/// Sets the maximum number of messages that a candidate can contain.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn set_max_upward_message_num_per_candidate(origin, new: u32) -> DispatchResult {
			ensure_root(origin)?;
			Self::update_config_member(|config| {
				sp_std::mem::replace(&mut config.max_upward_message_num_per_candidate, new) != new
			});
			Ok(())
		}
	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(_validators: &[ValidatorId], _queued: &[ValidatorId]) {
		if let Some(pending) = <Self as Store>::PendingConfig::take() {
			<Self as Store>::Config::set(pending);
		}
	}

	fn update_config_member(
		updater: impl FnOnce(&mut HostConfiguration<T::BlockNumber>) -> bool,
	) {
		let pending = <Self as Store>::PendingConfig::get();
		let mut prev = pending.unwrap_or_else(Self::config);

		if updater(&mut prev) {
			<Self as Store>::PendingConfig::set(Some(prev));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Initializer, Configuration, Origin};

	use frame_support::traits::{OnFinalize, OnInitialize};

	#[test]
	fn config_changes_on_session_boundary() {
		new_test_ext(Default::default()).execute_with(|| {
			let old_config = Configuration::config();
			let mut config = old_config.clone();
			config.validation_upgrade_delay = 100;

			assert!(old_config != config);

			<Configuration as Store>::PendingConfig::set(Some(config.clone()));

			Initializer::on_initialize(1);

			assert_eq!(Configuration::config(), old_config);
			assert_eq!(<Configuration as Store>::PendingConfig::get(), Some(config.clone()));

			Initializer::on_finalize(1);

			Configuration::initializer_on_new_session(&[], &[]);

			assert_eq!(Configuration::config(), config);
			assert!(<Configuration as Store>::PendingConfig::get().is_none());
		})
	}

	#[test]
	fn setting_pending_config_members() {
		new_test_ext(Default::default()).execute_with(|| {
			let new_config = HostConfiguration {
				validation_upgrade_frequency: 100,
				validation_upgrade_delay: 10,
				acceptance_period: 5,
				max_code_size: 100_000,
				max_head_data_size: 1_000,
				parathread_cores: 2,
				parathread_retries: 5,
				group_rotation_frequency: 20,
				chain_availability_period: 10,
				thread_availability_period: 8,
				scheduling_lookahead: 3,
				max_upward_queue_count: 1337,
				max_upward_queue_size: 228,
				max_downward_message_size: 2048,
				preferred_dispatchable_upward_messages_step_weight: 20000,
				max_upward_message_size: 448,
				max_upward_message_num_per_candidate: 5,
			};

			assert!(<Configuration as Store>::PendingConfig::get().is_none());

			Configuration::set_validation_upgrade_frequency(
				Origin::root(), new_config.validation_upgrade_frequency,
			).unwrap();
			Configuration::set_validation_upgrade_delay(
				Origin::root(), new_config.validation_upgrade_delay,
			).unwrap();
			Configuration::set_acceptance_period(
				Origin::root(), new_config.acceptance_period,
			).unwrap();
			Configuration::set_max_code_size(
				Origin::root(), new_config.max_code_size,
			).unwrap();
			Configuration::set_max_head_data_size(
				Origin::root(), new_config.max_head_data_size,
			).unwrap();
			Configuration::set_parathread_cores(
				Origin::root(), new_config.parathread_cores,
			).unwrap();
			Configuration::set_parathread_retries(
				Origin::root(), new_config.parathread_retries,
			).unwrap();
			Configuration::set_group_rotation_frequency(
				Origin::root(), new_config.group_rotation_frequency,
			).unwrap();
			Configuration::set_chain_availability_period(
				Origin::root(), new_config.chain_availability_period,
			).unwrap();
			Configuration::set_thread_availability_period(
				Origin::root(), new_config.thread_availability_period,
			).unwrap();
			Configuration::set_scheduling_lookahead(
				Origin::root(), new_config.scheduling_lookahead,
			).unwrap();
			Configuration::set_max_upward_queue_count(
				Origin::root(), new_config.max_upward_queue_count,
			).unwrap();
			Configuration::set_max_upward_queue_size(
				Origin::root(), new_config.max_upward_queue_size,
			).unwrap();
			Configuration::set_max_downward_message_size(
				Origin::root(), new_config.max_downward_message_size,
			).unwrap();
			Configuration::set_preferred_dispatchable_upward_messages_step_weight(
				Origin::root(), new_config.preferred_dispatchable_upward_messages_step_weight,
			).unwrap();
			Configuration::set_max_upward_message_size(
				Origin::root(), new_config.max_upward_message_size,
			).unwrap();
			Configuration::set_max_upward_message_num_per_candidate(
				Origin::root(), new_config.max_upward_message_num_per_candidate,
			).unwrap();

			assert_eq!(<Configuration as Store>::PendingConfig::get(), Some(new_config));
		})
	}

	#[test]
	fn non_root_cannot_set_config() {
		new_test_ext(Default::default()).execute_with(|| {
			assert!(Configuration::set_validation_upgrade_delay(Origin::signed(1), 100).is_err());
		});
	}

	#[test]
	fn setting_config_to_same_as_current_is_noop() {
		new_test_ext(Default::default()).execute_with(|| {
			Configuration::set_validation_upgrade_delay(Origin::root(), Default::default()).unwrap();
			assert!(<Configuration as Store>::PendingConfig::get().is_none())
		});
	}
}
