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

use super::*;
use crate::mock::{new_test_ext, Configuration, Origin, ParasShared, Test};
use frame_support::{assert_err, assert_ok};

fn on_new_session(session_index: SessionIndex) -> (HostConfiguration<u32>, HostConfiguration<u32>) {
	ParasShared::set_session_index(session_index);
	let SessionChangeOutcome { prev_config, new_config } =
		Configuration::initializer_on_new_session(&session_index);
	let new_config = new_config.unwrap_or_else(|| prev_config.clone());
	(prev_config, new_config)
}

#[test]
fn default_is_consistent() {
	new_test_ext(Default::default()).execute_with(|| {
		Configuration::config().panic_if_not_consistent();
	});
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
		assert_eq!(new_config, HostConfiguration { validation_upgrade_delay: 100, ..prev_config });
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
		config.validation_upgrade_cooldown = 100;
		assert!(old_config != config);

		assert_ok!(Configuration::set_validation_upgrade_delay(Origin::root(), 100));
		assert_ok!(Configuration::set_validation_upgrade_cooldown(Origin::root(), 100));
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
			validation_upgrade_cooldown: 99,
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
		assert_ok!(Configuration::set_validation_upgrade_cooldown(Origin::root(), 99));

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
			validation_upgrade_cooldown: 99,
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
		assert_ok!(Configuration::set_validation_upgrade_cooldown(Origin::root(), 99));
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
fn invariants() {
	new_test_ext(Default::default()).execute_with(|| {
		assert_err!(
			Configuration::set_max_code_size(Origin::root(), MAX_CODE_SIZE + 1),
			Error::<Test>::InvalidNewValue
		);

		assert_err!(
			Configuration::set_max_pov_size(Origin::root(), MAX_POV_SIZE + 1),
			Error::<Test>::InvalidNewValue
		);

		assert_err!(
			Configuration::set_max_head_data_size(Origin::root(), MAX_HEAD_DATA_SIZE + 1),
			Error::<Test>::InvalidNewValue
		);

		assert_err!(
			Configuration::set_chain_availability_period(Origin::root(), 0),
			Error::<Test>::InvalidNewValue
		);
		assert_err!(
			Configuration::set_thread_availability_period(Origin::root(), 0),
			Error::<Test>::InvalidNewValue
		);
		assert_err!(
			Configuration::set_no_show_slots(Origin::root(), 0),
			Error::<Test>::InvalidNewValue
		);

		<Configuration as Store>::ActiveConfig::put(HostConfiguration {
			chain_availability_period: 10,
			thread_availability_period: 8,
			minimum_validation_upgrade_delay: 11,
			..Default::default()
		});
		assert_err!(
			Configuration::set_chain_availability_period(Origin::root(), 12),
			Error::<Test>::InvalidNewValue
		);
		assert_err!(
			Configuration::set_thread_availability_period(Origin::root(), 12),
			Error::<Test>::InvalidNewValue
		);
		assert_err!(
			Configuration::set_minimum_validation_upgrade_delay(Origin::root(), 9),
			Error::<Test>::InvalidNewValue
		);

		assert_err!(
			Configuration::set_validation_upgrade_delay(Origin::root(), 0),
			Error::<Test>::InvalidNewValue
		);
	});
}

#[test]
fn consistency_bypass_works() {
	new_test_ext(Default::default()).execute_with(|| {
		assert_err!(
			Configuration::set_max_code_size(Origin::root(), MAX_CODE_SIZE + 1),
			Error::<Test>::InvalidNewValue
		);

		assert_ok!(Configuration::set_bypass_consistency_check(Origin::root(), true));
		assert_ok!(Configuration::set_max_code_size(Origin::root(), MAX_CODE_SIZE + 1));

		assert_eq!(
			Configuration::config().max_code_size,
			HostConfiguration::<u32>::default().max_code_size
		);

		on_new_session(1);
		on_new_session(2);

		assert_eq!(Configuration::config().max_code_size, MAX_CODE_SIZE + 1);
	});
}

#[test]
fn setting_pending_config_members() {
	new_test_ext(Default::default()).execute_with(|| {
		let new_config = HostConfiguration {
			validation_upgrade_cooldown: 100,
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
			ump_service_total_weight: Weight::from_ref_time(20000),
			max_upward_message_size: 448,
			max_upward_message_num_per_candidate: 5,
			hrmp_sender_deposit: 22,
			hrmp_recipient_deposit: 4905,
			hrmp_channel_max_capacity: 3921,
			hrmp_channel_max_total_size: 7687,
			hrmp_max_parachain_inbound_channels: 37,
			hrmp_max_parathread_inbound_channels: 19,
			hrmp_channel_max_message_size: 8192,
			hrmp_max_parachain_outbound_channels: 10,
			hrmp_max_parathread_outbound_channels: 20,
			hrmp_max_message_num_per_candidate: 20,
			ump_max_individual_weight: Weight::from_ref_time(909),
			pvf_checking_enabled: true,
			pvf_voting_ttl: 3,
			minimum_validation_upgrade_delay: 20,
		};

		Configuration::set_validation_upgrade_cooldown(
			Origin::root(),
			new_config.validation_upgrade_cooldown,
		)
		.unwrap();
		Configuration::set_validation_upgrade_delay(
			Origin::root(),
			new_config.validation_upgrade_delay,
		)
		.unwrap();
		Configuration::set_code_retention_period(Origin::root(), new_config.code_retention_period)
			.unwrap();
		Configuration::set_max_code_size(Origin::root(), new_config.max_code_size).unwrap();
		Configuration::set_max_pov_size(Origin::root(), new_config.max_pov_size).unwrap();
		Configuration::set_max_head_data_size(Origin::root(), new_config.max_head_data_size)
			.unwrap();
		Configuration::set_parathread_cores(Origin::root(), new_config.parathread_cores).unwrap();
		Configuration::set_parathread_retries(Origin::root(), new_config.parathread_retries)
			.unwrap();
		Configuration::set_group_rotation_frequency(
			Origin::root(),
			new_config.group_rotation_frequency,
		)
		.unwrap();
		// This comes out of order to satisfy the validity criteria for the chain and thread
		// availability periods.
		Configuration::set_minimum_validation_upgrade_delay(
			Origin::root(),
			new_config.minimum_validation_upgrade_delay,
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
		Configuration::set_scheduling_lookahead(Origin::root(), new_config.scheduling_lookahead)
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
		Configuration::set_n_delay_tranches(Origin::root(), new_config.n_delay_tranches).unwrap();
		Configuration::set_zeroth_delay_tranche_width(
			Origin::root(),
			new_config.zeroth_delay_tranche_width,
		)
		.unwrap();
		Configuration::set_needed_approvals(Origin::root(), new_config.needed_approvals).unwrap();
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
		Configuration::set_max_upward_queue_size(Origin::root(), new_config.max_upward_queue_size)
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
		Configuration::set_pvf_checking_enabled(Origin::root(), new_config.pvf_checking_enabled)
			.unwrap();
		Configuration::set_pvf_voting_ttl(Origin::root(), new_config.pvf_voting_ttl).unwrap();

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

	use primitives::v2::{well_known_keys, AbridgedHostConfiguration};

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
				hrmp_max_message_num_per_candidate: ground_truth.hrmp_max_message_num_per_candidate,
				validation_upgrade_cooldown: ground_truth.validation_upgrade_cooldown,
				validation_upgrade_delay: ground_truth.validation_upgrade_delay,
			},
		);
	});
}
