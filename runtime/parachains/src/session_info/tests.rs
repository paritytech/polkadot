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
use crate::{
	configuration::HostConfiguration,
	initializer::SessionChangeNotification,
	mock::{
		new_test_ext, Configuration, MockGenesisConfig, ParasShared, RuntimeOrigin, SessionInfo,
		System, Test,
	},
	util::take_active_subset,
};
use keyring::Sr25519Keyring;
use primitives::v2::{BlockNumber, ValidatorId, ValidatorIndex};

fn run_to_block(
	to: BlockNumber,
	new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
) {
	while System::block_number() < to {
		let b = System::block_number();

		SessionInfo::initializer_finalize();
		ParasShared::initializer_finalize();
		Configuration::initializer_finalize();

		if let Some(notification) = new_session(b + 1) {
			Configuration::initializer_on_new_session(&notification.session_index);
			ParasShared::initializer_on_new_session(
				notification.session_index,
				notification.random_seed,
				&notification.new_config,
				notification.validators.clone(),
			);
			SessionInfo::initializer_on_new_session(&notification);
		}

		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		Configuration::initializer_initialize(b + 1);
		ParasShared::initializer_initialize(b + 1);
		SessionInfo::initializer_initialize(b + 1);
	}
}

fn default_config() -> HostConfiguration<BlockNumber> {
	HostConfiguration {
		parathread_cores: 1,
		dispute_period: 2,
		needed_approvals: 3,
		..Default::default()
	}
}

fn genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	}
}

fn session_changes(n: BlockNumber) -> Option<SessionChangeNotification<BlockNumber>> {
	if n % 10 == 0 {
		Some(SessionChangeNotification { session_index: n / 10, ..Default::default() })
	} else {
		None
	}
}

fn new_session_every_block(n: BlockNumber) -> Option<SessionChangeNotification<BlockNumber>> {
	Some(SessionChangeNotification { session_index: n, ..Default::default() })
}

#[test]
fn session_pruning_is_based_on_dispute_period() {
	new_test_ext(genesis_config()).execute_with(|| {
		// Dispute period starts at 2
		let config = Configuration::config();
		assert_eq!(config.dispute_period, 2);

		// Move to session 10
		run_to_block(100, session_changes);
		// Earliest stored session is 10 - 2 = 8
		assert_eq!(EarliestStoredSession::<Test>::get(), 8);
		// Pruning works as expected
		assert!(Sessions::<Test>::get(7).is_none());
		assert!(Sessions::<Test>::get(8).is_some());
		assert!(Sessions::<Test>::get(9).is_some());

		// changing `dispute_period` works
		let dispute_period = 5;
		Configuration::set_dispute_period(RuntimeOrigin::root(), dispute_period).unwrap();

		// Dispute period does not automatically change
		let config = Configuration::config();
		assert_eq!(config.dispute_period, 2);
		// Two sessions later it will though
		run_to_block(120, session_changes);
		let config = Configuration::config();
		assert_eq!(config.dispute_period, 5);

		run_to_block(200, session_changes);
		assert_eq!(EarliestStoredSession::<Test>::get(), 20 - dispute_period);

		// Increase dispute period even more
		let new_dispute_period = 16;
		Configuration::set_dispute_period(RuntimeOrigin::root(), new_dispute_period).unwrap();

		run_to_block(210, session_changes);
		assert_eq!(EarliestStoredSession::<Test>::get(), 21 - dispute_period);

		// Two sessions later it kicks in
		run_to_block(220, session_changes);
		let config = Configuration::config();
		assert_eq!(config.dispute_period, 16);
		// Earliest session stays the same
		assert_eq!(EarliestStoredSession::<Test>::get(), 21 - dispute_period);

		// We still don't have enough stored sessions to start pruning
		run_to_block(300, session_changes);
		assert_eq!(EarliestStoredSession::<Test>::get(), 21 - dispute_period);

		// now we do
		run_to_block(420, session_changes);
		assert_eq!(EarliestStoredSession::<Test>::get(), 42 - new_dispute_period);
	})
}

#[test]
fn session_info_is_based_on_config() {
	new_test_ext(genesis_config()).execute_with(|| {
		run_to_block(1, new_session_every_block);
		let session = Sessions::<Test>::get(&1).unwrap();
		assert_eq!(session.needed_approvals, 3);

		// change some param
		Configuration::set_needed_approvals(RuntimeOrigin::root(), 42).unwrap();
		// 2 sessions later
		run_to_block(3, new_session_every_block);
		let session = Sessions::<Test>::get(&3).unwrap();
		assert_eq!(session.needed_approvals, 42);
	})
}

#[test]
fn session_info_active_subsets() {
	let unscrambled = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Eve,
	];

	let active_set = vec![ValidatorIndex(4), ValidatorIndex(0), ValidatorIndex(2)];

	let unscrambled_validators: Vec<ValidatorId> =
		unscrambled.iter().map(|v| v.public().into()).collect();
	let unscrambled_discovery: Vec<AuthorityDiscoveryId> =
		unscrambled.iter().map(|v| v.public().into()).collect();
	let unscrambled_assignment: Vec<AssignmentId> =
		unscrambled.iter().map(|v| v.public().into()).collect();

	let validators = take_active_subset(&active_set, &unscrambled_validators);

	new_test_ext(genesis_config()).execute_with(|| {
		ParasShared::set_active_validators_with_indices(active_set.clone(), validators.clone());

		assert_eq!(ParasShared::active_validator_indices(), active_set);

		AssignmentKeysUnsafe::<Test>::set(unscrambled_assignment.clone());
		crate::mock::set_discovery_authorities(unscrambled_discovery.clone());
		assert_eq!(<Test>::authorities(), unscrambled_discovery);

		// invoke directly, because `run_to_block` will invoke `Shared`	and clobber our
		// values.
		SessionInfo::initializer_on_new_session(&SessionChangeNotification {
			session_index: 1,
			validators: validators.clone(),
			..Default::default()
		});
		let session = Sessions::<Test>::get(&1).unwrap();

		assert_eq!(session.validators.to_vec(), validators);
		assert_eq!(
			session.discovery_keys,
			take_active_subset_and_inactive(&active_set, &unscrambled_discovery),
		);
		assert_eq!(
			session.assignment_keys,
			take_active_subset(&active_set, &unscrambled_assignment),
		);
	})
}
