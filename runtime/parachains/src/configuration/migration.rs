// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! A module that is responsible for migration of storage.

use crate::configuration::{self, Config, Pallet, Store};
use frame_support::{pallet_prelude::*, traits::StorageVersion, weights::Weight};
use frame_system::pallet_prelude::BlockNumberFor;
use sp_std::prelude::*;

/// The current storage version.
///
/// v0-v1: https://github.com/paritytech/polkadot/pull/3575
/// v1-v2: https://github.com/paritytech/polkadot/pull/4420
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
pub fn migrate_to_latest<T: Config>() -> Weight {
	let mut weight = 0;
	if StorageVersion::get::<Pallet<T>>() == 1 {
		weight += migrate_to_v2::<T>();
		StorageVersion::new(2).put::<Pallet<T>>();
	}
	weight
}

pub mod v1 {
	use super::*;
	use primitives::v1::{Balance, SessionIndex};

	// Copied over from configuration.rs @ 656dd280f266dc56bd0cf1dbe3ca232960912f34 and removed
	// all the comments.
	#[derive(
		parity_scale_codec::Encode, parity_scale_codec::Decode, scale_info::TypeInfo, Debug, Clone,
	)]
	pub struct HostConfiguration<BlockNumber> {
		pub max_code_size: u32,
		pub max_head_data_size: u32,
		pub max_upward_queue_count: u32,
		pub max_upward_queue_size: u32,
		pub max_upward_message_size: u32,
		pub max_upward_message_num_per_candidate: u32,
		pub hrmp_max_message_num_per_candidate: u32,
		pub validation_upgrade_frequency: BlockNumber,
		pub validation_upgrade_delay: BlockNumber,
		pub max_pov_size: u32,
		pub max_downward_message_size: u32,
		pub ump_service_total_weight: Weight,
		pub hrmp_max_parachain_outbound_channels: u32,
		pub hrmp_max_parathread_outbound_channels: u32,
		pub hrmp_sender_deposit: Balance,
		pub hrmp_recipient_deposit: Balance,
		pub hrmp_channel_max_capacity: u32,
		pub hrmp_channel_max_total_size: u32,
		pub hrmp_max_parachain_inbound_channels: u32,
		pub hrmp_max_parathread_inbound_channels: u32,
		pub hrmp_channel_max_message_size: u32,
		pub code_retention_period: BlockNumber,
		pub parathread_cores: u32,
		pub parathread_retries: u32,
		pub group_rotation_frequency: BlockNumber,
		pub chain_availability_period: BlockNumber,
		pub thread_availability_period: BlockNumber,
		pub scheduling_lookahead: u32,
		pub max_validators_per_core: Option<u32>,
		pub max_validators: Option<u32>,
		pub dispute_period: SessionIndex,
		pub dispute_post_conclusion_acceptance_period: BlockNumber,
		pub dispute_max_spam_slots: u32,
		pub dispute_conclusion_by_time_out_period: BlockNumber,
		pub no_show_slots: u32,
		pub n_delay_tranches: u32,
		pub zeroth_delay_tranche_width: u32,
		pub needed_approvals: u32,
		pub relay_vrf_modulo_samples: u32,
		pub ump_max_individual_weight: Weight,
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
				ump_max_individual_weight: 20 *
					frame_support::weights::constants::WEIGHT_PER_MILLIS,
			}
		}
	}
}

pub fn migrate_to_v2<T: Config>() -> Weight {
	// Unusual formatting is justified:
	// - make it easier to verify that fields assign what they supposed to assign.
	// - this code is transient and will be removed after all migrations are done.
	// - this code is important enough to optimize for legibility sacrificing consistency.
	#[rustfmt::skip]
	let translate =
		|pre: v1::HostConfiguration<BlockNumberFor<T>>| -> configuration::HostConfiguration<BlockNumberFor<T>>
	{
		super::HostConfiguration {

max_code_size                            : pre.max_code_size,
max_head_data_size                       : pre.max_head_data_size,
max_upward_queue_count                   : pre.max_upward_queue_count,
max_upward_queue_size                    : pre.max_upward_queue_size,
max_upward_message_size                  : pre.max_upward_message_size,
max_upward_message_num_per_candidate     : pre.max_upward_message_num_per_candidate,
hrmp_max_message_num_per_candidate       : pre.hrmp_max_message_num_per_candidate,
validation_upgrade_cooldown              : pre.validation_upgrade_frequency,
validation_upgrade_delay                 : pre.validation_upgrade_delay,
max_pov_size                             : pre.max_pov_size,
max_downward_message_size                : pre.max_downward_message_size,
ump_service_total_weight                 : pre.ump_service_total_weight,
hrmp_max_parachain_outbound_channels     : pre.hrmp_max_parachain_outbound_channels,
hrmp_max_parathread_outbound_channels    : pre.hrmp_max_parathread_outbound_channels,
hrmp_sender_deposit                      : pre.hrmp_sender_deposit,
hrmp_recipient_deposit                   : pre.hrmp_recipient_deposit,
hrmp_channel_max_capacity                : pre.hrmp_channel_max_capacity,
hrmp_channel_max_total_size              : pre.hrmp_channel_max_total_size,
hrmp_max_parachain_inbound_channels      : pre.hrmp_max_parachain_inbound_channels,
hrmp_max_parathread_inbound_channels     : pre.hrmp_max_parathread_inbound_channels,
hrmp_channel_max_message_size            : pre.hrmp_channel_max_message_size,
code_retention_period                    : pre.code_retention_period,
parathread_cores                         : pre.parathread_cores,
parathread_retries                       : pre.parathread_retries,
group_rotation_frequency                 : pre.group_rotation_frequency,
chain_availability_period                : pre.chain_availability_period,
thread_availability_period               : pre.thread_availability_period,
scheduling_lookahead                     : pre.scheduling_lookahead,
max_validators_per_core                  : pre.max_validators_per_core,
max_validators                           : pre.max_validators,
dispute_period                           : pre.dispute_period,
dispute_post_conclusion_acceptance_period: pre.dispute_post_conclusion_acceptance_period,
dispute_max_spam_slots                   : pre.dispute_max_spam_slots,
dispute_conclusion_by_time_out_period    : pre.dispute_conclusion_by_time_out_period,
no_show_slots                            : pre.no_show_slots,
n_delay_tranches                         : pre.n_delay_tranches,
zeroth_delay_tranche_width               : pre.zeroth_delay_tranche_width,
needed_approvals                         : pre.needed_approvals,
relay_vrf_modulo_samples                 : pre.relay_vrf_modulo_samples,
ump_max_individual_weight                : pre.ump_max_individual_weight,

pvf_checking_enabled: false,
pvf_voting_ttl: 2u32.into(),
minimum_validation_upgrade_delay: pre.chain_availability_period + 10u32.into(),
		}
	};

	let mut weight = 0;

	// First, ActiveConfig

	weight += T::DbWeight::get().reads_writes(1, 1);
	if let Err(_) = <Pallet<T> as Store>::ActiveConfig::translate(|pre| pre.map(translate)) {
		// `Err` is returned when the pre-migration type cannot be deserialized. This
		// cannot happen if the migration runs correctly, i.e. against the expected version.
		//
		// This happening almost surely will lead to a panic somewhere else. Corruption seems
		// to be unlikely to be caused by this. So we just log. Maybe it'll work out still?
		log::error!(
			target: configuration::LOG_TARGET,
			"unexpected error when performing translation of the configuration type during storage upgrade to v2."
		);
	}

	// Second, PendingConfig -> PendingConfigs

	weight += T::DbWeight::get().reads(2);
	let current_session_index = crate::shared::Pallet::<T>::session_index();
	let scheduled_session = crate::shared::Pallet::<T>::scheduled_session();
	let mut pending_configs = Vec::new();

	for session_index in current_session_index..=scheduled_session {
		weight += T::DbWeight::get().reads(1);
		if let Some(pending_config) = <Pallet<T> as Store>::PendingConfig::get(session_index) {
			pending_configs.push((session_index, translate(pending_config)));
		}
	}

	weight += T::DbWeight::get().writes(1);
	<Pallet<T> as Store>::PendingConfigs::put(&pending_configs);

	weight
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Test};

	#[test]
	fn v1_deserialized_from_actual_data() {
		// Fetched at Kusama 10,320,500 (0x3b2c305d01bd4adf1973d32a2d55ca1260a55eea8dfb3168e317c57f2841fdf1)
		//
		// This exceeds the maximal line width length, but that's fine, since this is not code and
		// doesn't need to be read and also leaving it as one line allows to easily copy it.
		let raw_config = hex_literal::hex!["0000a000005000000a00000000c8000000c800000a0000000a00000040380000580200000000500000c8000000e87648170000000a00000000000000005039278c0400000000000000000000005039278c0400000000000000000000e8030000009001000a00000000000000009001008070000000000000000000000a0000000a0000000a00000001000000010500000001c8000000060000005802000002000000580200000200000059000000000000001e0000002800000000c817a804000000"];

		let v1 = v1::HostConfiguration::<primitives::v1::BlockNumber>::decode(&mut &raw_config[..])
			.unwrap();

		// We check only a sample of the values here. If we missed any fields or messed up data types
		// that would skew all the fields coming after.
		assert_eq!(v1.max_code_size, 10_485_760);
		assert_eq!(v1.validation_upgrade_frequency, 14_400);
		assert_eq!(v1.max_pov_size, 5_242_880);
		assert_eq!(v1.hrmp_channel_max_message_size, 102_400);
		assert_eq!(v1.dispute_max_spam_slots, 2);
		assert_eq!(v1.n_delay_tranches, 89);
		assert_eq!(v1.ump_max_individual_weight, 20_000_000_000);
	}

	#[test]
	fn test_migrate_to_v2() {
		// Host configuration has lots of fields. However, in this migration we add only a couple of
		// fields. The most important part to check are a couple of the last fields. We also pick
		// extra fields to check arbitrarily, e.g. depending on their position (i.e. the middle) and
		// also their type.
		//
		// We specify only the picked fields and the rest should be provided by the `Default`
		// implementation. That implementation is copied over between the two types and should work
		// fine.
		let v1 = v1::HostConfiguration::<primitives::v1::BlockNumber> {
			ump_max_individual_weight: 0x71616e6f6e0au64,
			needed_approvals: 69,
			thread_availability_period: 55,
			hrmp_recipient_deposit: 1337,
			max_pov_size: 1111,
			chain_availability_period: 33,
			..Default::default()
		};
		let pending_configs_v1 = vec![
			(
				1,
				v1::HostConfiguration::<primitives::v1::BlockNumber> {
					n_delay_tranches: 150,
					..v1.clone()
				},
			),
			(
				2,
				v1::HostConfiguration::<primitives::v1::BlockNumber> {
					max_validators_per_core: Some(33),
					..v1.clone()
				},
			),
			(
				3,
				v1::HostConfiguration::<primitives::v1::BlockNumber> {
					parathread_retries: 11,
					..v1.clone()
				},
			),
		];

		new_test_ext(Default::default()).execute_with(|| {
			// Implant the v1 data in the state.
			frame_support::storage::unhashed::put_raw(
				&configuration::ActiveConfig::<Test>::hashed_key(),
				&v1.encode(),
			);
			for (session_index, pending_config) in &pending_configs_v1 {
				frame_support::storage::unhashed::put_raw(
					&configuration::PendingConfig::<Test>::hashed_key_for(session_index),
					&pending_config.encode(),
				);
			}

			// Assume the session index 1.
			crate::shared::Pallet::<Test>::set_session_index(1);

			migrate_to_v2::<Test>();

			let v2 = configuration::ActiveConfig::<Test>::get();

			assert_correct_translation(v1, v2);
			let pending_configs_v2 = configuration::PendingConfigs::<Test>::get();
			assert_eq!(pending_configs_v1.len(), pending_configs_v2.len());
			for ((session_index_v1, pending_config_v1), (session_index_v2, pending_configs_v2)) in
				pending_configs_v1.into_iter().zip(pending_configs_v2.into_iter())
			{
				assert_eq!(session_index_v1, session_index_v2);
				assert_correct_translation(pending_config_v1, pending_configs_v2);
			}
		});

		// The same motivation as for the migration code. See `migrate_to_v2`.
		#[rustfmt::skip]
		fn assert_correct_translation(
			v1: v1::HostConfiguration<primitives::v1::BlockNumber>, 
			v2: configuration::HostConfiguration<primitives::v1::BlockNumber>
		) {
			assert_eq!(v1.max_code_size                            , v2.max_code_size);
			assert_eq!(v1.max_head_data_size                       , v2.max_head_data_size);
			assert_eq!(v1.max_upward_queue_count                   , v2.max_upward_queue_count);
			assert_eq!(v1.max_upward_queue_size                    , v2.max_upward_queue_size);
			assert_eq!(v1.max_upward_message_size                  , v2.max_upward_message_size);
			assert_eq!(v1.max_upward_message_num_per_candidate     , v2.max_upward_message_num_per_candidate);
			assert_eq!(v1.hrmp_max_message_num_per_candidate       , v2.hrmp_max_message_num_per_candidate);
			assert_eq!(v1.validation_upgrade_frequency             , v2.validation_upgrade_cooldown);
			assert_eq!(v1.validation_upgrade_delay                 , v2.validation_upgrade_delay);
			assert_eq!(v1.max_pov_size                             , v2.max_pov_size);
			assert_eq!(v1.max_downward_message_size                , v2.max_downward_message_size);
			assert_eq!(v1.ump_service_total_weight                 , v2.ump_service_total_weight);
			assert_eq!(v1.hrmp_max_parachain_outbound_channels     , v2.hrmp_max_parachain_outbound_channels);
			assert_eq!(v1.hrmp_max_parathread_outbound_channels    , v2.hrmp_max_parathread_outbound_channels);
			assert_eq!(v1.hrmp_sender_deposit                      , v2.hrmp_sender_deposit);
			assert_eq!(v1.hrmp_recipient_deposit                   , v2.hrmp_recipient_deposit);
			assert_eq!(v1.hrmp_channel_max_capacity                , v2.hrmp_channel_max_capacity);
			assert_eq!(v1.hrmp_channel_max_total_size              , v2.hrmp_channel_max_total_size);
			assert_eq!(v1.hrmp_max_parachain_inbound_channels      , v2.hrmp_max_parachain_inbound_channels);
			assert_eq!(v1.hrmp_max_parathread_inbound_channels     , v2.hrmp_max_parathread_inbound_channels);
			assert_eq!(v1.hrmp_channel_max_message_size            , v2.hrmp_channel_max_message_size);
			assert_eq!(v1.code_retention_period                    , v2.code_retention_period);
			assert_eq!(v1.parathread_cores                         , v2.parathread_cores);
			assert_eq!(v1.parathread_retries                       , v2.parathread_retries);
			assert_eq!(v1.group_rotation_frequency                 , v2.group_rotation_frequency);
			assert_eq!(v1.chain_availability_period                , v2.chain_availability_period);
			assert_eq!(v1.thread_availability_period               , v2.thread_availability_period);
			assert_eq!(v1.scheduling_lookahead                     , v2.scheduling_lookahead);
			assert_eq!(v1.max_validators_per_core                  , v2.max_validators_per_core);
			assert_eq!(v1.max_validators                           , v2.max_validators);
			assert_eq!(v1.dispute_period                           , v2.dispute_period);
			assert_eq!(v1.dispute_post_conclusion_acceptance_period, v2.dispute_post_conclusion_acceptance_period);
			assert_eq!(v1.dispute_max_spam_slots                   , v2.dispute_max_spam_slots);
			assert_eq!(v1.dispute_conclusion_by_time_out_period    , v2.dispute_conclusion_by_time_out_period);
			assert_eq!(v1.no_show_slots                            , v2.no_show_slots);
			assert_eq!(v1.n_delay_tranches                         , v2.n_delay_tranches);
			assert_eq!(v1.zeroth_delay_tranche_width               , v2.zeroth_delay_tranche_width);
			assert_eq!(v1.needed_approvals                         , v2.needed_approvals);
			assert_eq!(v1.relay_vrf_modulo_samples                 , v2.relay_vrf_modulo_samples);
			assert_eq!(v1.ump_max_individual_weight                , v2.ump_max_individual_weight);

			assert_eq!(v2.pvf_checking_enabled, false);
			assert_eq!(v2.pvf_voting_ttl, 2);
			assert_eq!(v2.minimum_validation_upgrade_delay, 43);
		}
	}
}
