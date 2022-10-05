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

use crate::configuration::{self, Config, Pallet, Store, MAX_POV_SIZE};
use frame_support::{
	pallet_prelude::*,
	traits::StorageVersion,
	weights::{OldWeight, Weight},
};
use frame_system::pallet_prelude::BlockNumberFor;

/// The current storage version.
///
/// v0-v1: <https://github.com/paritytech/polkadot/pull/3575>
/// v1-v2: <https://github.com/paritytech/polkadot/pull/4420>
/// v2-v3: <https://github.com/paritytech/polkadot/pull/6091>
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(3);

pub mod v3 {
	use super::*;
	use frame_support::traits::OnRuntimeUpgrade;
	use primitives::v2::{Balance, SessionIndex};

	// Copied over from configuration.rs @ de9e147695b9f1be8bd44e07861a31e483c8343a and removed
	// all the comments, and changed the Weight struct to OldWeight
	#[derive(parity_scale_codec::Encode, parity_scale_codec::Decode, Debug)]
	pub struct OldHostConfiguration<BlockNumber> {
		pub max_code_size: u32,
		pub max_head_data_size: u32,
		pub max_upward_queue_count: u32,
		pub max_upward_queue_size: u32,
		pub max_upward_message_size: u32,
		pub max_upward_message_num_per_candidate: u32,
		pub hrmp_max_message_num_per_candidate: u32,
		pub validation_upgrade_cooldown: BlockNumber,
		pub validation_upgrade_delay: BlockNumber,
		pub max_pov_size: u32,
		pub max_downward_message_size: u32,
		pub ump_service_total_weight: OldWeight,
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
		pub ump_max_individual_weight: OldWeight,
		pub pvf_checking_enabled: bool,
		pub pvf_voting_ttl: SessionIndex,
		pub minimum_validation_upgrade_delay: BlockNumber,
	}

	impl<BlockNumber: Default + From<u32>> Default for OldHostConfiguration<BlockNumber> {
		fn default() -> Self {
			Self {
				group_rotation_frequency: 1u32.into(),
				chain_availability_period: 1u32.into(),
				thread_availability_period: 1u32.into(),
				no_show_slots: 1u32.into(),
				validation_upgrade_cooldown: Default::default(),
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
				ump_service_total_weight: OldWeight(Default::default()),
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
				ump_max_individual_weight: OldWeight(
					frame_support::weights::constants::WEIGHT_PER_MILLIS.ref_time() * 20,
				),
				pvf_checking_enabled: false,
				pvf_voting_ttl: 2u32.into(),
				minimum_validation_upgrade_delay: 2.into(),
			}
		}
	}

	pub struct MigrateToV3<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV3<T> {
		fn on_runtime_upgrade() -> Weight {
			if StorageVersion::get::<Pallet<T>>() == 2 {
				let weight_consumed = migrate_to_v3::<T>();

				log::info!(target: configuration::LOG_TARGET, "MigrateToV3 executed successfully");
				STORAGE_VERSION.put::<Pallet<T>>();

				weight_consumed
			} else {
				log::warn!(target: configuration::LOG_TARGET, "MigrateToV3 should be removed.");
				T::DbWeight::get().reads(1)
			}
		}
	}
}

fn migrate_to_v3<T: Config>() -> Weight {
	// Unusual formatting is justified:
	// - make it easier to verify that fields assign what they supposed to assign.
	// - this code is transient and will be removed after all migrations are done.
	// - this code is important enough to optimize for legibility sacrificing consistency.
	#[rustfmt::skip]
	let translate =
		|pre: v3::OldHostConfiguration<BlockNumberFor<T>>| ->
configuration::HostConfiguration<BlockNumberFor<T>>
	{
		super::HostConfiguration {
max_code_size                            : pre.max_code_size,
max_head_data_size                       : pre.max_head_data_size,
max_upward_queue_count                   : pre.max_upward_queue_count,
max_upward_queue_size                    : pre.max_upward_queue_size,
max_upward_message_size                  : pre.max_upward_message_size,
max_upward_message_num_per_candidate     : pre.max_upward_message_num_per_candidate,
hrmp_max_message_num_per_candidate       : pre.hrmp_max_message_num_per_candidate,
validation_upgrade_cooldown              : pre.validation_upgrade_cooldown,
validation_upgrade_delay                 : pre.validation_upgrade_delay,
max_pov_size                             : pre.max_pov_size,
max_downward_message_size                : pre.max_downward_message_size,
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
pvf_checking_enabled                     : pre.pvf_checking_enabled,
pvf_voting_ttl                           : pre.pvf_voting_ttl,
minimum_validation_upgrade_delay         : pre.minimum_validation_upgrade_delay,

ump_service_total_weight: Weight::from_ref_time(pre.ump_service_total_weight.0).set_proof_size(MAX_POV_SIZE as u64),
ump_max_individual_weight: Weight::from_ref_time(pre.ump_max_individual_weight.0).set_proof_size(MAX_POV_SIZE as u64),
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
			"unexpected error when performing translation of the configuration type during storage upgrade to v2."
		);
	}

	T::DbWeight::get().reads_writes(1, 1)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Test};

	#[test]
	fn v2_deserialized_from_actual_data() {
		// Fetched at Kusama 14,703,780 (0x3b2c305d01bd4adf1973d32a2d55ca1260a55eea8dfb3168e317c57f2841fdf1)
		//
		// This exceeds the maximal line width length, but that's fine, since this is not code and
		// doesn't need to be read and also leaving it as one line allows to easily copy it.
		let raw_config = hex_literal::hex!["0000a000005000000a00000000c8000000c800000a0000000a000000100e0000580200000000500000c8000000e87648170000001e00000000000000005039278c0400000000000000000000005039278c0400000000000000000000e8030000009001001e00000000000000009001008070000000000000000000000a0000000a0000000a00000001000000010500000001c8000000060000005802000002000000580200000200000059000000000000001e0000002800000000c817a804000000000200000014000000"];

		let v2 =
			v3::OldHostConfiguration::<primitives::v2::BlockNumber>::decode(&mut &raw_config[..])
				.unwrap();

		// We check only a sample of the values here. If we missed any fields or messed up data types
		// that would skew all the fields coming after.
		assert_eq!(v2.max_code_size, 10_485_760);
		assert_eq!(v2.validation_upgrade_cooldown, 3600);
		assert_eq!(v2.max_pov_size, 5_242_880);
		assert_eq!(v2.hrmp_channel_max_message_size, 102_400);
		assert_eq!(v2.dispute_max_spam_slots, 2);
		assert_eq!(v2.n_delay_tranches, 89);
		assert_eq!(v2.ump_max_individual_weight, OldWeight(20_000_000_000));
		assert_eq!(v2.minimum_validation_upgrade_delay, 20);
	}

	#[test]
	fn test_migrate_to_v3() {
		// Host configuration has lots of fields. However, in this migration we add only a couple of
		// fields. The most important part to check are a couple of the last fields. We also pick
		// extra fields to check arbitrarily, e.g. depending on their position (i.e. the middle) and
		// also their type.
		//
		// We specify only the picked fields and the rest should be provided by the `Default`
		// implementation. That implementation is copied over between the two types and should work
		// fine.
		let v2 = v3::OldHostConfiguration::<primitives::v2::BlockNumber> {
			ump_max_individual_weight: OldWeight(0x71616e6f6e0au64),
			needed_approvals: 69,
			thread_availability_period: 55,
			hrmp_recipient_deposit: 1337,
			max_pov_size: 1111,
			chain_availability_period: 33,
			minimum_validation_upgrade_delay: 20,
			..Default::default()
		};

		new_test_ext(Default::default()).execute_with(|| {
			// Implant the v2 version in the state.
			frame_support::storage::unhashed::put_raw(
				&configuration::ActiveConfig::<Test>::hashed_key(),
				&v2.encode(),
			);

			migrate_to_v3::<Test>();

			let v3 = configuration::ActiveConfig::<Test>::get();

			#[rustfmt::skip]
			{
				assert_eq!(v2.max_code_size                            , v3.max_code_size);
				assert_eq!(v2.max_head_data_size                       , v3.max_head_data_size);
				assert_eq!(v2.max_upward_queue_count                   , v3.max_upward_queue_count);
				assert_eq!(v2.max_upward_queue_size                    , v3.max_upward_queue_size);
				assert_eq!(v2.max_upward_message_size                  , v3.max_upward_message_size);
				assert_eq!(v2.max_upward_message_num_per_candidate     , v3.max_upward_message_num_per_candidate);
				assert_eq!(v2.hrmp_max_message_num_per_candidate       , v3.hrmp_max_message_num_per_candidate);
				assert_eq!(v2.validation_upgrade_cooldown              , v3.validation_upgrade_cooldown);
				assert_eq!(v2.validation_upgrade_delay                 , v3.validation_upgrade_delay);
				assert_eq!(v2.max_pov_size                             , v3.max_pov_size);
				assert_eq!(v2.max_downward_message_size                , v3.max_downward_message_size);
				assert_eq!(v2.hrmp_max_parachain_outbound_channels     , v3.hrmp_max_parachain_outbound_channels);
				assert_eq!(v2.hrmp_max_parathread_outbound_channels    , v3.hrmp_max_parathread_outbound_channels);
				assert_eq!(v2.hrmp_sender_deposit                      , v3.hrmp_sender_deposit);
				assert_eq!(v2.hrmp_recipient_deposit                   , v3.hrmp_recipient_deposit);
				assert_eq!(v2.hrmp_channel_max_capacity                , v3.hrmp_channel_max_capacity);
				assert_eq!(v2.hrmp_channel_max_total_size              , v3.hrmp_channel_max_total_size);
				assert_eq!(v2.hrmp_max_parachain_inbound_channels      , v3.hrmp_max_parachain_inbound_channels);
				assert_eq!(v2.hrmp_max_parathread_inbound_channels     , v3.hrmp_max_parathread_inbound_channels);
				assert_eq!(v2.hrmp_channel_max_message_size            , v3.hrmp_channel_max_message_size);
				assert_eq!(v2.code_retention_period                    , v3.code_retention_period);
				assert_eq!(v2.parathread_cores                         , v3.parathread_cores);
				assert_eq!(v2.parathread_retries                       , v3.parathread_retries);
				assert_eq!(v2.group_rotation_frequency                 , v3.group_rotation_frequency);
				assert_eq!(v2.chain_availability_period                , v3.chain_availability_period);
				assert_eq!(v2.thread_availability_period               , v3.thread_availability_period);
				assert_eq!(v2.scheduling_lookahead                     , v3.scheduling_lookahead);
				assert_eq!(v2.max_validators_per_core                  , v3.max_validators_per_core);
				assert_eq!(v2.max_validators                           , v3.max_validators);
				assert_eq!(v2.dispute_period                           , v3.dispute_period);
				assert_eq!(v2.dispute_post_conclusion_acceptance_period, v3.dispute_post_conclusion_acceptance_period);
				assert_eq!(v2.dispute_max_spam_slots                   , v3.dispute_max_spam_slots);
				assert_eq!(v2.dispute_conclusion_by_time_out_period    , v3.dispute_conclusion_by_time_out_period);
				assert_eq!(v2.no_show_slots                            , v3.no_show_slots);
				assert_eq!(v2.n_delay_tranches                         , v3.n_delay_tranches);
				assert_eq!(v2.zeroth_delay_tranche_width               , v3.zeroth_delay_tranche_width);
				assert_eq!(v2.needed_approvals                         , v3.needed_approvals);
				assert_eq!(v2.relay_vrf_modulo_samples                 , v3.relay_vrf_modulo_samples);
				assert_eq!(v2.pvf_checking_enabled                     , v3.pvf_checking_enabled);
				assert_eq!(v2.pvf_voting_ttl                           , v3.pvf_voting_ttl);
				assert_eq!(v2.minimum_validation_upgrade_delay         , v3.minimum_validation_upgrade_delay);

				assert_eq!(v2.ump_service_total_weight, OldWeight(v3.ump_service_total_weight.ref_time()));
				assert_eq!(v2.ump_max_individual_weight, OldWeight(v3.ump_max_individual_weight.ref_time()));
				assert_eq!(v3.ump_service_total_weight.proof_size(), MAX_POV_SIZE as u64);
				assert_eq!(v3.ump_max_individual_weight.proof_size(), MAX_POV_SIZE as u64);
			}; // ; makes this a statement. `rustfmt::skip` cannot be put on an expression.
		});
	}
}
