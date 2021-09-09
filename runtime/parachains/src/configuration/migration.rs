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

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

/// Call this during an next runtime upgrade for this module.
pub fn on_runtime_upgrade<T: Config>() -> Weight {
	let mut weight = 0;

	if StorageVersion::get::<Pallet<T>>() == 0 {
		weight += migrate_to_v1::<T>();
		StorageVersion::new(1).put::<Pallet<T>>();
	}

	weight
}

mod v0 {
	use super::*;
	use primitives::v1::{Balance, SessionIndex};

	#[derive(parity_scale_codec::Encode, parity_scale_codec::Decode, Debug)]
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
		pub hrmp_open_request_ttl: u32,
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
	}

	impl<BlockNumber: Default + From<u32>> Default for HostConfiguration<BlockNumber> {
		fn default() -> Self {
			HostConfiguration {
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
				hrmp_open_request_ttl: Default::default(),
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
			}
		}
	}
}

fn migrate_to_v1<T: Config>() -> Weight {
	// Unusual formatting is justified:
	// - make it easier to verify that fields assign what they supposed to assign.
	// - this code is transient and will be removed after all migrations are done.
	// - this code is important enough to optimize for legibility sacrificing consistency.
	#[rustfmt::skip]
	let translate =
		|pre: v0::HostConfiguration<BlockNumberFor<T>>| -> configuration::HostConfiguration<BlockNumberFor<T>>
	{
		super::HostConfiguration {

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
hrmp_open_request_ttl                    : pre.hrmp_open_request_ttl,
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

ump_max_individual_weight: <configuration::HostConfiguration<BlockNumberFor<T>>>::default().ump_max_individual_weight,
		}
	};

	if let Err(_) = <Pallet<T> as Store>::ActiveConfig::translate(|pre| pre.map(translate)) {
		// `Err` is returned when the pre-migration type cannot be deserialized. This
		// cannot happen if the migration run correctly, i.e. against the expected version.
		//
		// This happening almost sure will lead to a panic somewhere else. Corruption seems
		// to be unlikely to be caused by this. So we just log. Maybe it'll work out still?
		log::error!(
			target: configuration::LOG_TARGET,
			"unexpected error when performing translation of the configuration type during storage upgrade to v1."
		);
	}

	T::DbWeight::get().reads_writes(1, 1)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Test};

	#[test]
	fn test_migrate_to_v1() {
		// Host configuration has lots of fields. However, in this migration we only add a single
		// field. The most important part to check are a couple of the last fields. We also pick
		// extra fields to check arbitrary, e.g. depending on their position (i.e. the middle) and
		// also their type.
		//
		// We specify only the picked fields and the rest should be provided by the `Default`
		// implementation. That implementation is copied over between the two types and should work
		// fine.
		let v0 = v0::HostConfiguration::<primitives::v1::BlockNumber> {
			relay_vrf_modulo_samples: 0xFEEDBEEFu32,
			needed_approvals: 69,
			thread_availability_period: 55,
			hrmp_recipient_deposit: 1337,
			max_pov_size: 1111,
			..Default::default()
		};

		new_test_ext(Default::default()).execute_with(|| {
			// Implant the v0 version in the state.
			frame_support::storage::unhashed::put_raw(
				&configuration::ActiveConfig::<Test>::hashed_key(),
				&v0.encode(),
			);

			migrate_to_v1::<Test>();

			let v1 = configuration::ActiveConfig::<Test>::get();

			// The same motivation as for the migration code. See `migrate_to_v1`.
			#[rustfmt::skip]
			{
				assert_eq!(v0.max_code_size                            , v1.max_code_size);
				assert_eq!(v0.max_head_data_size                       , v1.max_head_data_size);
				assert_eq!(v0.max_upward_queue_count                   , v1.max_upward_queue_count);
				assert_eq!(v0.max_upward_queue_size                    , v1.max_upward_queue_size);
				assert_eq!(v0.max_upward_message_size                  , v1.max_upward_message_size);
				assert_eq!(v0.max_upward_message_num_per_candidate     , v1.max_upward_message_num_per_candidate);
				assert_eq!(v0.hrmp_max_message_num_per_candidate       , v1.hrmp_max_message_num_per_candidate);
				assert_eq!(v0.validation_upgrade_frequency             , v1.validation_upgrade_frequency);
				assert_eq!(v0.validation_upgrade_delay                 , v1.validation_upgrade_delay);
				assert_eq!(v0.max_pov_size                             , v1.max_pov_size);
				assert_eq!(v0.max_downward_message_size                , v1.max_downward_message_size);
				assert_eq!(v0.ump_service_total_weight                 , v1.ump_service_total_weight);
				assert_eq!(v0.hrmp_max_parachain_outbound_channels     , v1.hrmp_max_parachain_outbound_channels);
				assert_eq!(v0.hrmp_max_parathread_outbound_channels    , v1.hrmp_max_parathread_outbound_channels);
				assert_eq!(v0.hrmp_open_request_ttl                    , v1.hrmp_open_request_ttl);
				assert_eq!(v0.hrmp_sender_deposit                      , v1.hrmp_sender_deposit);
				assert_eq!(v0.hrmp_recipient_deposit                   , v1.hrmp_recipient_deposit);
				assert_eq!(v0.hrmp_channel_max_capacity                , v1.hrmp_channel_max_capacity);
				assert_eq!(v0.hrmp_channel_max_total_size              , v1.hrmp_channel_max_total_size);
				assert_eq!(v0.hrmp_max_parachain_inbound_channels      , v1.hrmp_max_parachain_inbound_channels);
				assert_eq!(v0.hrmp_max_parathread_inbound_channels     , v1.hrmp_max_parathread_inbound_channels);
				assert_eq!(v0.hrmp_channel_max_message_size            , v1.hrmp_channel_max_message_size);
				assert_eq!(v0.code_retention_period                    , v1.code_retention_period);
				assert_eq!(v0.parathread_cores                         , v1.parathread_cores);
				assert_eq!(v0.parathread_retries                       , v1.parathread_retries);
				assert_eq!(v0.group_rotation_frequency                 , v1.group_rotation_frequency);
				assert_eq!(v0.chain_availability_period                , v1.chain_availability_period);
				assert_eq!(v0.thread_availability_period               , v1.thread_availability_period);
				assert_eq!(v0.scheduling_lookahead                     , v1.scheduling_lookahead);
				assert_eq!(v0.max_validators_per_core                  , v1.max_validators_per_core);
				assert_eq!(v0.max_validators                           , v1.max_validators);
				assert_eq!(v0.dispute_period                           , v1.dispute_period);
				assert_eq!(v0.dispute_post_conclusion_acceptance_period, v1.dispute_post_conclusion_acceptance_period);
				assert_eq!(v0.dispute_max_spam_slots                   , v1.dispute_max_spam_slots);
				assert_eq!(v0.dispute_conclusion_by_time_out_period    , v1.dispute_conclusion_by_time_out_period);
				assert_eq!(v0.no_show_slots                            , v1.no_show_slots);
				assert_eq!(v0.n_delay_tranches                         , v1.n_delay_tranches);
				assert_eq!(v0.zeroth_delay_tranche_width               , v1.zeroth_delay_tranche_width);
				assert_eq!(v0.needed_approvals                         , v1.needed_approvals);
				assert_eq!(v0.relay_vrf_modulo_samples                 , v1.relay_vrf_modulo_samples);

				assert_eq!(v1.ump_max_individual_weight, 10_000_000_000);
			}; // ; makes this a statement. `rustfmt::skip` cannot be put on an expression.
		});
	}
}
