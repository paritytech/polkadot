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

use crate::configuration::{Config, Pallet};
use frame_support::{
	traits::{Get, StorageVersion},
	weights::Weight,
};

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
pub fn migrate_to_latest<T: Config>() -> Weight {
	let mut weight: Weight = 0;

	if StorageVersion::get::<Pallet<T>>() == 1 {
		weight = weight
			.saturating_add(v2::migrate::<T>())
			.saturating_add(T::DbWeight::get().writes(1));
		StorageVersion::new(2).put::<Pallet<T>>();
	}

	weight
}

mod v1 {
	use frame_support::weights::Weight;
	use parity_scale_codec::{Decode, Encode};
	use primitives::v1::{Balance, SessionIndex};

	#[derive(Debug, Default, Encode, Decode)]
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
}

mod v2 {
	use super::v1;
	use crate::configuration::{self, Config, Pallet, Store};
	use frame_support::{traits::Get, weights::Weight};
	use frame_system::pallet_prelude::BlockNumberFor;

	pub fn migrate<T: Config>() -> Weight {
		#[rustfmt::skip]
		let translate =
			|pre: v1::HostConfiguration<BlockNumberFor<T>>| -> configuration::HostConfiguration<BlockNumberFor<T>>
		{
			let new = configuration::HostConfiguration::default();
			configuration::HostConfiguration {
				max_code_size                            : pre.max_code_size,
				max_head_data_size                       : pre.max_head_data_size,
				max_upward_queue_count                   : pre.max_upward_queue_count,
				max_upward_queue_size                    : pre.max_upward_queue_size,
				max_upward_message_size                  : pre.max_upward_message_size,
				max_upward_message_num_per_candidate     : pre.max_upward_message_num_per_candidate,
				hrmp_max_message_num_per_candidate       : pre.hrmp_max_message_num_per_candidate,
				validation_upgrade_frequency             : pre.validation_upgrade_frequency,
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

				min_upgrade_delay                        : new.min_upgrade_delay,
				pvf_prechecking_bypass                   : new.pvf_prechecking_bypass,
				pvf_upgrade_delay                        : new.pvf_upgrade_delay,
				pvf_voting_ttl                           : new.pvf_voting_ttl,
			}
		};

		if let Err(_) = <Pallet<T> as Store>::ActiveConfig::translate(|pre| pre.map(translate)) {
			// `Err` is returned when the pre-migration type cannot be deserialized. This
			// cannot happen if the migration runs correctly, i.e. against the expected version.
			//
			// This happening almost surely will lead to a panic somewhere else. Corruption seems
			// to be unlikely to be caused by this. So we just log. Maybe it'll work out still?
			log::error!(
				target: configuration::LOG_TARGET,
				"unexpected error when performing translation of the configuration type during storage upgrade to v1."
			);
		}

		// TODO: does the number of fields affect these values?
		T::DbWeight::get().reads_writes(1, 1)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		configuration,
		mock::{new_test_ext, Test},
	};
	use parity_scale_codec::{Decode, Encode};
	use primitives::v1::BlockNumber;

	#[test]
	fn v1_deserialized_from_actual_data() {
		// TODO: is this test necessary?

		// Fetched at Kusama 9,863,403.
		//
		// This exceeds the maximal line width length, but that's fine, since this is not code and
		// doesn't need to be read and also leaving it as one line allows to easily copy it.
		let raw_config = hex_literal::hex!["0000a000005000000a00000000c8000000c800000a0000000a00000040380000580200000000500000c8000000e87648170000000a00000000000000c09e5d9a2f3d00000000000000000000c09e5d9a2f3d00000000000000000000e8030000009001000a00000000000000009001008070000000000000000000000a0000000a0000000a00000001000000010500000001c8000000060000005802000002000000580200000200000059000000000000001e0000002800000000c817a804000000"];

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
		assert_eq!(v1.relay_vrf_modulo_samples, 40);
		assert_eq!(v1.ump_max_individual_weight, 20_000_000_000);
	}

	#[test]
	fn test_migrate_to_v2() {
		let old = v1::HostConfiguration::<BlockNumber> {
			validation_upgrade_frequency: 123,
			ump_service_total_weight: u64::MAX,
			max_validators_per_core: Some(999),
			ump_max_individual_weight: 812,
			max_pov_size: 1111,
			..Default::default()
		};

		new_test_ext(Default::default()).execute_with(|| {
			frame_support::storage::unhashed::put_raw(
				&configuration::ActiveConfig::<Test>::hashed_key(),
				&old.encode(),
			);

			v2::migrate::<Test>();

			let new = configuration::ActiveConfig::<Test>::get();

			#[rustfmt::skip]
			{
				assert_eq!(old.max_code_size                            , new.max_code_size);
				assert_eq!(old.max_head_data_size                       , new.max_head_data_size);
				assert_eq!(old.max_upward_queue_count                   , new.max_upward_queue_count);
				assert_eq!(old.max_upward_queue_size                    , new.max_upward_queue_size);
				assert_eq!(old.max_upward_message_size                  , new.max_upward_message_size);
				assert_eq!(old.max_upward_message_num_per_candidate     , new.max_upward_message_num_per_candidate);
				assert_eq!(old.hrmp_max_message_num_per_candidate       , new.hrmp_max_message_num_per_candidate);
				assert_eq!(old.validation_upgrade_frequency             , new.validation_upgrade_frequency);
				assert_eq!(old.validation_upgrade_delay                 , new.validation_upgrade_delay);
				assert_eq!(old.max_pov_size                             , new.max_pov_size);
				assert_eq!(old.max_downward_message_size                , new.max_downward_message_size);
				assert_eq!(old.ump_service_total_weight                 , new.ump_service_total_weight);
				assert_eq!(old.hrmp_max_parachain_outbound_channels     , new.hrmp_max_parachain_outbound_channels);
				assert_eq!(old.hrmp_max_parathread_outbound_channels    , new.hrmp_max_parathread_outbound_channels);
				assert_eq!(old.hrmp_sender_deposit                      , new.hrmp_sender_deposit);
				assert_eq!(old.hrmp_recipient_deposit                   , new.hrmp_recipient_deposit);
				assert_eq!(old.hrmp_channel_max_capacity                , new.hrmp_channel_max_capacity);
				assert_eq!(old.hrmp_channel_max_total_size              , new.hrmp_channel_max_total_size);
				assert_eq!(old.hrmp_max_parachain_inbound_channels      , new.hrmp_max_parachain_inbound_channels);
				assert_eq!(old.hrmp_max_parathread_inbound_channels     , new.hrmp_max_parathread_inbound_channels);
				assert_eq!(old.hrmp_channel_max_message_size            , new.hrmp_channel_max_message_size);
				assert_eq!(old.code_retention_period                    , new.code_retention_period);
				assert_eq!(old.parathread_cores                         , new.parathread_cores);
				assert_eq!(old.parathread_retries                       , new.parathread_retries);
				assert_eq!(old.group_rotation_frequency                 , new.group_rotation_frequency);
				assert_eq!(old.chain_availability_period                , new.chain_availability_period);
				assert_eq!(old.thread_availability_period               , new.thread_availability_period);
				assert_eq!(old.scheduling_lookahead                     , new.scheduling_lookahead);
				assert_eq!(old.max_validators_per_core                  , new.max_validators_per_core);
				assert_eq!(old.max_validators                           , new.max_validators);
				assert_eq!(old.dispute_period                           , new.dispute_period);
				assert_eq!(old.dispute_post_conclusion_acceptance_period, new.dispute_post_conclusion_acceptance_period);
				assert_eq!(old.dispute_max_spam_slots                   , new.dispute_max_spam_slots);
				assert_eq!(old.dispute_conclusion_by_time_out_period    , new.dispute_conclusion_by_time_out_period);
				assert_eq!(old.no_show_slots                            , new.no_show_slots);
				assert_eq!(old.n_delay_tranches                         , new.n_delay_tranches);
				assert_eq!(old.zeroth_delay_tranche_width               , new.zeroth_delay_tranche_width);
				assert_eq!(old.needed_approvals                         , new.needed_approvals);
				assert_eq!(old.relay_vrf_modulo_samples                 , new.relay_vrf_modulo_samples);
				assert_eq!(old.ump_max_individual_weight                , new.ump_max_individual_weight);

				// TODO: add tests for new values after figuring out their defaults.
			}
		});
	}
}
