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

use super::*;
use frame_support::{assert_err, assert_ok, assert_storage_noop};
use keyring::Sr25519Keyring;
use primitives::v2::{BlockNumber, ValidatorId, PARACHAIN_KEY_TYPE_ID};
use sc_keystore::LocalKeystore;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use std::sync::Arc;
use test_helpers::{dummy_head_data, dummy_validation_code};

use crate::{
	configuration::HostConfiguration,
	mock::{
		new_test_ext, Configuration, MockGenesisConfig, Paras, ParasShared, RuntimeOrigin, System,
		Test,
	},
};

static VALIDATORS: &[Sr25519Keyring] = &[
	Sr25519Keyring::Alice,
	Sr25519Keyring::Bob,
	Sr25519Keyring::Charlie,
	Sr25519Keyring::Dave,
	Sr25519Keyring::Ferdie,
];

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn sign_and_include_pvf_check_statement(stmt: PvfCheckStatement) {
	let validators = &[
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];
	let signature = validators[stmt.validator_index.0 as usize].sign(&stmt.signing_payload());
	Paras::include_pvf_check_statement(None.into(), stmt, signature.into()).unwrap();
}

fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
	let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
	for validator in VALIDATORS.iter() {
		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			PARACHAIN_KEY_TYPE_ID,
			Some(&validator.to_seed()),
		)
		.unwrap();
	}
	let validator_pubkeys = validator_pubkeys(VALIDATORS);

	while System::block_number() < to {
		let b = System::block_number();
		Paras::initializer_finalize(b);
		ParasShared::initializer_finalize();
		if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
			let mut session_change_notification = SessionChangeNotification::default();
			session_change_notification.session_index = ParasShared::session_index() + 1;
			session_change_notification.validators = validator_pubkeys.clone();
			ParasShared::initializer_on_new_session(
				session_change_notification.session_index,
				session_change_notification.random_seed,
				&session_change_notification.new_config,
				session_change_notification.validators.clone(),
			);
			ParasShared::set_active_validators_ascending(validator_pubkeys.clone());
			Paras::initializer_on_new_session(&session_change_notification);
		}
		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		ParasShared::initializer_initialize(b + 1);
		Paras::initializer_initialize(b + 1);
	}
}

fn upgrade_at(
	expected_at: BlockNumber,
	activated_at: BlockNumber,
) -> ReplacementTimes<BlockNumber> {
	ReplacementTimes { expected_at, activated_at }
}

fn check_code_is_stored(validation_code: &ValidationCode) {
	assert!(<Paras as Store>::CodeByHashRefs::get(validation_code.hash()) != 0);
	assert!(<Paras as Store>::CodeByHash::contains_key(validation_code.hash()));
}

fn check_code_is_not_stored(validation_code: &ValidationCode) {
	assert!(!<Paras as Store>::CodeByHashRefs::contains_key(validation_code.hash()));
	assert!(!<Paras as Store>::CodeByHash::contains_key(validation_code.hash()));
}

/// An utility for checking that certain events were deposited.
struct EventValidator {
	events: Vec<
		frame_system::EventRecord<
			<Test as frame_system::Config>::RuntimeEvent,
			primitives::v2::Hash,
		>,
	>,
}

impl EventValidator {
	fn new() -> Self {
		Self { events: Vec::new() }
	}

	fn started(&mut self, code: &ValidationCode, id: ParaId) -> &mut Self {
		self.events.push(frame_system::EventRecord {
			phase: frame_system::Phase::Initialization,
			event: Event::PvfCheckStarted(code.hash(), id).into(),
			topics: vec![],
		});
		self
	}

	fn rejected(&mut self, code: &ValidationCode, id: ParaId) -> &mut Self {
		self.events.push(frame_system::EventRecord {
			phase: frame_system::Phase::Initialization,
			event: Event::PvfCheckRejected(code.hash(), id).into(),
			topics: vec![],
		});
		self
	}

	fn accepted(&mut self, code: &ValidationCode, id: ParaId) -> &mut Self {
		self.events.push(frame_system::EventRecord {
			phase: frame_system::Phase::Initialization,
			event: Event::PvfCheckAccepted(code.hash(), id).into(),
			topics: vec![],
		});
		self
	}

	fn check(&self) {
		assert_eq!(&frame_system::Pallet::<Test>::events(), &self.events);
	}
}

#[test]
fn para_past_code_pruning_works_correctly() {
	let mut past_code = ParaPastCodeMeta::default();
	past_code.note_replacement(10u32, 10);
	past_code.note_replacement(20, 25);
	past_code.note_replacement(30, 35);

	let old = past_code.clone();
	assert!(past_code.prune_up_to(9).collect::<Vec<_>>().is_empty());
	assert_eq!(old, past_code);

	assert_eq!(past_code.prune_up_to(10).collect::<Vec<_>>(), vec![10]);
	assert_eq!(
		past_code,
		ParaPastCodeMeta {
			upgrade_times: vec![upgrade_at(20, 25), upgrade_at(30, 35)],
			last_pruned: Some(10),
		}
	);

	assert!(past_code.prune_up_to(21).collect::<Vec<_>>().is_empty());

	assert_eq!(past_code.prune_up_to(26).collect::<Vec<_>>(), vec![20]);
	assert_eq!(
		past_code,
		ParaPastCodeMeta { upgrade_times: vec![upgrade_at(30, 35)], last_pruned: Some(25) }
	);

	past_code.note_replacement(40, 42);
	past_code.note_replacement(50, 53);
	past_code.note_replacement(60, 66);

	assert_eq!(
		past_code,
		ParaPastCodeMeta {
			upgrade_times: vec![
				upgrade_at(30, 35),
				upgrade_at(40, 42),
				upgrade_at(50, 53),
				upgrade_at(60, 66)
			],
			last_pruned: Some(25),
		}
	);

	assert_eq!(past_code.prune_up_to(60).collect::<Vec<_>>(), vec![30, 40, 50]);
	assert_eq!(
		past_code,
		ParaPastCodeMeta { upgrade_times: vec![upgrade_at(60, 66)], last_pruned: Some(53) }
	);

	assert_eq!(past_code.most_recent_change(), Some(60));
	assert_eq!(past_code.prune_up_to(66).collect::<Vec<_>>(), vec![60]);

	assert_eq!(past_code, ParaPastCodeMeta { upgrade_times: Vec::new(), last_pruned: Some(66) });
}

#[test]
fn schedule_para_init_rejects_empty_code() {
	new_test_ext(MockGenesisConfig::default()).execute_with(|| {
		assert_err!(
			Paras::schedule_para_initialize(
				1000.into(),
				ParaGenesisArgs {
					parachain: false,
					genesis_head: dummy_head_data(),
					validation_code: ValidationCode(vec![]),
				}
			),
			Error::<Test>::CannotOnboard,
		);

		assert_ok!(Paras::schedule_para_initialize(
			1000.into(),
			ParaGenesisArgs {
				parachain: false,
				genesis_head: dummy_head_data(),
				validation_code: ValidationCode(vec![1]),
			}
		));
	});
}

#[test]
fn para_past_code_pruning_in_initialize() {
	let code_retention_period = 10;
	let paras = vec![
		(
			0u32.into(),
			ParaGenesisArgs {
				parachain: true,
				genesis_head: dummy_head_data(),
				validation_code: dummy_validation_code(),
			},
		),
		(
			1u32.into(),
			ParaGenesisArgs {
				parachain: false,
				genesis_head: dummy_head_data(),
				validation_code: dummy_validation_code(),
			},
		),
	];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { code_retention_period, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let id = ParaId::from(0u32);
		let at_block: BlockNumber = 10;
		let included_block: BlockNumber = 12;
		let validation_code = ValidationCode(vec![4, 5, 6]);

		Paras::increase_code_ref(&validation_code.hash(), &validation_code);
		<Paras as Store>::PastCodeHash::insert(&(id, at_block), &validation_code.hash());
		<Paras as Store>::PastCodePruning::put(&vec![(id, included_block)]);

		{
			let mut code_meta = Paras::past_code_meta(&id);
			code_meta.note_replacement(at_block, included_block);
			<Paras as Store>::PastCodeMeta::insert(&id, &code_meta);
		}

		let pruned_at: BlockNumber = included_block + code_retention_period + 1;
		assert_eq!(
			<Paras as Store>::PastCodeHash::get(&(id, at_block)),
			Some(validation_code.hash())
		);
		check_code_is_stored(&validation_code);

		run_to_block(pruned_at - 1, None);
		assert_eq!(
			<Paras as Store>::PastCodeHash::get(&(id, at_block)),
			Some(validation_code.hash())
		);
		assert_eq!(Paras::past_code_meta(&id).most_recent_change(), Some(at_block));
		check_code_is_stored(&validation_code);

		run_to_block(pruned_at, None);
		assert!(<Paras as Store>::PastCodeHash::get(&(id, at_block)).is_none());
		assert!(Paras::past_code_meta(&id).most_recent_change().is_none());
		check_code_is_not_stored(&validation_code);
	});
}

#[test]
fn note_new_head_sets_head() {
	let code_retention_period = 10;
	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: dummy_validation_code(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { code_retention_period, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let id_a = ParaId::from(0u32);

		assert_eq!(Paras::para_head(&id_a), Some(dummy_head_data()));

		Paras::note_new_head(id_a, vec![1, 2, 3].into(), 0);

		assert_eq!(Paras::para_head(&id_a), Some(vec![1, 2, 3].into()));
	});
}

#[test]
fn note_past_code_sets_up_pruning_correctly() {
	let code_retention_period = 10;
	let paras = vec![
		(
			0u32.into(),
			ParaGenesisArgs {
				parachain: true,
				genesis_head: dummy_head_data(),
				validation_code: dummy_validation_code(),
			},
		),
		(
			1u32.into(),
			ParaGenesisArgs {
				parachain: false,
				genesis_head: dummy_head_data(),
				validation_code: dummy_validation_code(),
			},
		),
	];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { code_retention_period, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let id_a = ParaId::from(0u32);
		let id_b = ParaId::from(1u32);

		Paras::note_past_code(id_a, 10, 12, ValidationCode(vec![1, 2, 3]).hash());
		Paras::note_past_code(id_b, 20, 23, ValidationCode(vec![4, 5, 6]).hash());

		assert_eq!(<Paras as Store>::PastCodePruning::get(), vec![(id_a, 12), (id_b, 23)]);
		assert_eq!(
			Paras::past_code_meta(&id_a),
			ParaPastCodeMeta { upgrade_times: vec![upgrade_at(10, 12)], last_pruned: None }
		);
		assert_eq!(
			Paras::past_code_meta(&id_b),
			ParaPastCodeMeta { upgrade_times: vec![upgrade_at(20, 23)], last_pruned: None }
		);
	});
}

#[test]
fn code_upgrade_applied_after_delay() {
	let code_retention_period = 10;
	let validation_upgrade_delay = 5;
	let validation_upgrade_cooldown = 10;

	let original_code = ValidationCode(vec![1, 2, 3]);
	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: original_code.clone(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				validation_upgrade_cooldown,
				pvf_checking_enabled: false,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		check_code_is_stored(&original_code);

		let para_id = ParaId::from(0);
		let new_code = ValidationCode(vec![4, 5, 6]);

		run_to_block(2, None);
		assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));

		let expected_at = {
			// this parablock is in the context of block 1.
			let expected_at = 1 + validation_upgrade_delay;
			let next_possible_upgrade_at = 1 + validation_upgrade_cooldown;
			Paras::schedule_code_upgrade(para_id, new_code.clone(), 1, &Configuration::config());
			Paras::note_new_head(para_id, Default::default(), 1);

			assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
			assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
			assert_eq!(<Paras as Store>::UpcomingUpgrades::get(), vec![(para_id, expected_at)]);
			assert_eq!(
				<Paras as Store>::UpgradeCooldowns::get(),
				vec![(para_id, next_possible_upgrade_at)]
			);
			assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));
			check_code_is_stored(&original_code);
			check_code_is_stored(&new_code);

			expected_at
		};

		run_to_block(expected_at, None);

		// the candidate is in the context of the parent of `expected_at`,
		// thus does not trigger the code upgrade.
		{
			Paras::note_new_head(para_id, Default::default(), expected_at - 1);

			assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
			assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
			assert_eq!(
				<Paras as Store>::UpgradeGoAheadSignal::get(&para_id),
				Some(UpgradeGoAhead::GoAhead)
			);
			assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));
			check_code_is_stored(&original_code);
			check_code_is_stored(&new_code);
		}

		run_to_block(expected_at + 1, None);

		// the candidate is in the context of `expected_at`, and triggers
		// the upgrade.
		{
			Paras::note_new_head(para_id, Default::default(), expected_at);

			assert_eq!(Paras::past_code_meta(&para_id).most_recent_change(), Some(expected_at));
			assert_eq!(
				<Paras as Store>::PastCodeHash::get(&(para_id, expected_at)),
				Some(original_code.hash()),
			);
			assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
			assert!(<Paras as Store>::FutureCodeHash::get(&para_id).is_none());
			assert!(<Paras as Store>::UpgradeGoAheadSignal::get(&para_id).is_none());
			assert_eq!(Paras::current_code(&para_id), Some(new_code.clone()));
			check_code_is_stored(&original_code);
			check_code_is_stored(&new_code);
		}
	});
}

#[test]
fn code_upgrade_applied_after_delay_even_when_late() {
	let code_retention_period = 10;
	let validation_upgrade_delay = 5;
	let validation_upgrade_cooldown = 10;

	let original_code = ValidationCode(vec![1, 2, 3]);
	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: original_code.clone(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				validation_upgrade_cooldown,
				pvf_checking_enabled: false,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let para_id = ParaId::from(0);
		let new_code = ValidationCode(vec![4, 5, 6]);

		run_to_block(2, None);
		assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));

		let expected_at = {
			// this parablock is in the context of block 1.
			let expected_at = 1 + validation_upgrade_delay;
			let next_possible_upgrade_at = 1 + validation_upgrade_cooldown;
			Paras::schedule_code_upgrade(para_id, new_code.clone(), 1, &Configuration::config());
			Paras::note_new_head(para_id, Default::default(), 1);

			assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
			assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
			assert_eq!(<Paras as Store>::UpcomingUpgrades::get(), vec![(para_id, expected_at)]);
			assert_eq!(
				<Paras as Store>::UpgradeCooldowns::get(),
				vec![(para_id, next_possible_upgrade_at)]
			);
			assert!(<Paras as Store>::UpgradeGoAheadSignal::get(&para_id).is_none());
			assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));

			expected_at
		};

		run_to_block(expected_at + 1 + 4, None);

		// the candidate is in the context of the first descendant of `expected_at`, and triggers
		// the upgrade.
		{
			// The signal should be set to go-ahead until the new head is actually processed.
			assert_eq!(
				<Paras as Store>::UpgradeGoAheadSignal::get(&para_id),
				Some(UpgradeGoAhead::GoAhead),
			);

			Paras::note_new_head(para_id, Default::default(), expected_at + 4);

			assert_eq!(Paras::past_code_meta(&para_id).most_recent_change(), Some(expected_at));

			assert_eq!(
				<Paras as Store>::PastCodeHash::get(&(para_id, expected_at)),
				Some(original_code.hash()),
			);
			assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
			assert!(<Paras as Store>::FutureCodeHash::get(&para_id).is_none());
			assert!(<Paras as Store>::UpgradeGoAheadSignal::get(&para_id).is_none());
			assert_eq!(Paras::current_code(&para_id), Some(new_code.clone()));
		}
	});
}

#[test]
fn submit_code_change_when_not_allowed_is_err() {
	let code_retention_period = 10;
	let validation_upgrade_delay = 7;
	let validation_upgrade_cooldown = 100;

	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: vec![1, 2, 3].into(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				validation_upgrade_cooldown,
				pvf_checking_enabled: false,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let para_id = ParaId::from(0);
		let new_code = ValidationCode(vec![4, 5, 6]);
		let newer_code = ValidationCode(vec![4, 5, 6, 7]);

		run_to_block(1, None);
		Paras::schedule_code_upgrade(para_id, new_code.clone(), 1, &Configuration::config());
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&para_id),
			Some(1 + validation_upgrade_delay)
		);
		assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
		check_code_is_stored(&new_code);

		// We expect that if an upgrade is signalled while there is already one pending we just
		// ignore it. Note that this is only true from perspective of this module.
		run_to_block(2, None);
		assert!(!Paras::can_upgrade_validation_code(para_id));
		Paras::schedule_code_upgrade(para_id, newer_code.clone(), 2, &Configuration::config());
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&para_id),
			Some(1 + validation_upgrade_delay), // did not change since the same assertion from the last time.
		);
		assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
		check_code_is_not_stored(&newer_code);
	});
}

#[test]
fn upgrade_restriction_elapsed_doesnt_mean_can_upgrade() {
	// Situation: parachain scheduled upgrade but it doesn't produce any candidate after
	// `expected_at`. When `validation_upgrade_cooldown` elapsed the parachain produces a
	// candidate that tries to upgrade the code.
	//
	// In the current code this is not allowed: the upgrade should be consumed first. This is
	// rather an artifact of the current implementation and not necessarily something we want
	// to keep in the future.
	//
	// This test exists that this is not accidentially changed.

	let code_retention_period = 10;
	let validation_upgrade_delay = 7;
	let validation_upgrade_cooldown = 30;

	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: vec![1, 2, 3].into(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				validation_upgrade_cooldown,
				pvf_checking_enabled: false,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let para_id = 0u32.into();
		let new_code = ValidationCode(vec![4, 5, 6]);
		let newer_code = ValidationCode(vec![4, 5, 6, 7]);

		run_to_block(1, None);
		Paras::schedule_code_upgrade(para_id, new_code.clone(), 0, &Configuration::config());
		Paras::note_new_head(para_id, dummy_head_data(), 0);
		assert_eq!(
			<Paras as Store>::UpgradeRestrictionSignal::get(&para_id),
			Some(UpgradeRestriction::Present),
		);
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&para_id),
			Some(0 + validation_upgrade_delay)
		);
		assert!(!Paras::can_upgrade_validation_code(para_id));

		run_to_block(31, None);
		assert!(<Paras as Store>::UpgradeRestrictionSignal::get(&para_id).is_none());

		// Note the para still cannot upgrade the validation code.
		assert!(!Paras::can_upgrade_validation_code(para_id));

		// And scheduling another upgrade does not do anything. `expected_at` is still the same.
		Paras::schedule_code_upgrade(para_id, newer_code.clone(), 30, &Configuration::config());
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&para_id),
			Some(0 + validation_upgrade_delay)
		);
	});
}

#[test]
fn full_parachain_cleanup_storage() {
	let code_retention_period = 20;
	let validation_upgrade_delay = 1 + 5;

	let original_code = ValidationCode(vec![1, 2, 3]);
	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: original_code.clone(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				pvf_checking_enabled: false,
				minimum_validation_upgrade_delay: 2,
				// Those are not relevant to this test. However, HostConfiguration is still a
				// subject for the consistency check.
				chain_availability_period: 1,
				thread_availability_period: 1,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		check_code_is_stored(&original_code);

		let para_id = ParaId::from(0);
		let new_code = ValidationCode(vec![4, 5, 6]);

		run_to_block(2, None);
		assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));
		check_code_is_stored(&original_code);

		let expected_at = {
			// this parablock is in the context of block 1.
			let expected_at = 1 + validation_upgrade_delay;
			Paras::schedule_code_upgrade(para_id, new_code.clone(), 1, &Configuration::config());
			Paras::note_new_head(para_id, Default::default(), 1);

			assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
			assert_eq!(<Paras as Store>::FutureCodeHash::get(&para_id), Some(new_code.hash()));
			assert_eq!(Paras::current_code(&para_id), Some(original_code.clone()));
			check_code_is_stored(&original_code);
			check_code_is_stored(&new_code);

			expected_at
		};

		// Cannot offboard while an upgrade is pending.
		assert_err!(Paras::schedule_para_cleanup(para_id), Error::<Test>::CannotOffboard);

		// Enact the upgrade.
		//
		// For that run to block #7 and submit a new head.
		assert_eq!(expected_at, 7);
		run_to_block(7, None);
		assert_eq!(<frame_system::Pallet<Test>>::block_number(), 7);
		Paras::note_new_head(para_id, Default::default(), expected_at);

		assert_ok!(Paras::schedule_para_cleanup(para_id));

		// run to block #10, with a 2 session changes at the end of the block 7 & 8 (so 8 and 9
		// observe the new sessions).
		run_to_block(10, Some(vec![8, 9]));

		// cleaning up the parachain should place the current parachain code
		// into the past code buffer & schedule cleanup.
		//
		// Why 7 and 8? See above, the clean up scheduled above was processed at the block 8.
		// The initial upgrade was enacted at the block 7.
		assert_eq!(Paras::past_code_meta(&para_id).most_recent_change(), Some(8));
		assert_eq!(<Paras as Store>::PastCodeHash::get(&(para_id, 8)), Some(new_code.hash()));
		assert_eq!(<Paras as Store>::PastCodePruning::get(), vec![(para_id, 7), (para_id, 8)]);
		check_code_is_stored(&original_code);
		check_code_is_stored(&new_code);

		// any future upgrades haven't been used to validate yet, so those
		// are cleaned up immediately.
		assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
		assert!(<Paras as Store>::FutureCodeHash::get(&para_id).is_none());
		assert!(Paras::current_code(&para_id).is_none());

		// run to do the final cleanup
		let cleaned_up_at = 8 + code_retention_period + 1;
		run_to_block(cleaned_up_at, None);

		// now the final cleanup: last past code cleaned up, and this triggers meta cleanup.
		assert_eq!(Paras::past_code_meta(&para_id), Default::default());
		assert!(<Paras as Store>::PastCodeHash::get(&(para_id, 7)).is_none());
		assert!(<Paras as Store>::PastCodeHash::get(&(para_id, 8)).is_none());
		assert!(<Paras as Store>::PastCodePruning::get().is_empty());
		check_code_is_not_stored(&original_code);
		check_code_is_not_stored(&new_code);
	});
}

#[test]
fn para_incoming_at_session() {
	let code_a = ValidationCode(vec![2]);
	let code_b = ValidationCode(vec![1]);
	let code_c = ValidationCode(vec![3]);

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { pvf_checking_enabled: true, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		run_to_block(1, Some(vec![1]));

		let b = ParaId::from(525);
		let a = ParaId::from(999);
		let c = ParaId::from(333);

		assert_ok!(Paras::schedule_para_initialize(
			b,
			ParaGenesisArgs {
				parachain: true,
				genesis_head: vec![1].into(),
				validation_code: code_b.clone(),
			},
		));

		assert_ok!(Paras::schedule_para_initialize(
			a,
			ParaGenesisArgs {
				parachain: false,
				genesis_head: vec![2].into(),
				validation_code: code_a.clone(),
			},
		));

		assert_ok!(Paras::schedule_para_initialize(
			c,
			ParaGenesisArgs {
				parachain: true,
				genesis_head: vec![3].into(),
				validation_code: code_c.clone(),
			},
		));

		IntoIterator::into_iter([0, 1, 2, 3])
			.map(|i| PvfCheckStatement {
				accept: true,
				subject: code_a.hash(),
				session_index: 1,
				validator_index: i.into(),
			})
			.for_each(sign_and_include_pvf_check_statement);

		IntoIterator::into_iter([1, 2, 3, 4])
			.map(|i| PvfCheckStatement {
				accept: true,
				subject: code_b.hash(),
				session_index: 1,
				validator_index: i.into(),
			})
			.for_each(sign_and_include_pvf_check_statement);

		IntoIterator::into_iter([0, 2, 3, 4])
			.map(|i| PvfCheckStatement {
				accept: true,
				subject: code_c.hash(),
				session_index: 1,
				validator_index: i.into(),
			})
			.for_each(sign_and_include_pvf_check_statement);

		assert_eq!(<Paras as Store>::ActionsQueue::get(Paras::scheduled_session()), vec![c, b, a],);

		// Lifecycle is tracked correctly
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&a), Some(ParaLifecycle::Onboarding));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&b), Some(ParaLifecycle::Onboarding));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&c), Some(ParaLifecycle::Onboarding));

		// run to block without session change.
		run_to_block(2, None);

		assert_eq!(Paras::parachains(), Vec::new());
		assert_eq!(<Paras as Store>::ActionsQueue::get(Paras::scheduled_session()), vec![c, b, a],);

		// Lifecycle is tracked correctly
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&a), Some(ParaLifecycle::Onboarding));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&b), Some(ParaLifecycle::Onboarding));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&c), Some(ParaLifecycle::Onboarding));

		// Two sessions pass, so action queue is triggered
		run_to_block(4, Some(vec![3, 4]));

		assert_eq!(Paras::parachains(), vec![c, b]);
		assert_eq!(<Paras as Store>::ActionsQueue::get(Paras::scheduled_session()), Vec::new());

		// Lifecycle is tracked correctly
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&a), Some(ParaLifecycle::Parathread));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&b), Some(ParaLifecycle::Parachain));
		assert_eq!(<Paras as Store>::ParaLifecycles::get(&c), Some(ParaLifecycle::Parachain));

		assert_eq!(Paras::current_code(&a), Some(vec![2].into()));
		assert_eq!(Paras::current_code(&b), Some(vec![1].into()));
		assert_eq!(Paras::current_code(&c), Some(vec![3].into()));
	})
}

#[test]
fn code_hash_at_returns_up_to_end_of_code_retention_period() {
	let code_retention_period = 10;
	let validation_upgrade_delay = 2;

	let paras = vec![(
		0u32.into(),
		ParaGenesisArgs {
			parachain: true,
			genesis_head: dummy_head_data(),
			validation_code: vec![1, 2, 3].into(),
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				code_retention_period,
				validation_upgrade_delay,
				pvf_checking_enabled: false,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let para_id = ParaId::from(0);
		let old_code: ValidationCode = vec![1, 2, 3].into();
		let new_code: ValidationCode = vec![4, 5, 6].into();
		Paras::schedule_code_upgrade(para_id, new_code.clone(), 0, &Configuration::config());

		// The new validation code can be applied but a new parablock hasn't gotten in yet,
		// so the old code should still be current.
		run_to_block(3, None);
		assert_eq!(Paras::current_code(&para_id), Some(old_code.clone()));

		run_to_block(10, None);
		Paras::note_new_head(para_id, Default::default(), 7);

		assert_eq!(Paras::past_code_meta(&para_id).upgrade_times, vec![upgrade_at(2, 10)]);
		assert_eq!(Paras::current_code(&para_id), Some(new_code.clone()));

		// Make sure that the old code is available **before** the code retion period passes.
		run_to_block(10 + code_retention_period, None);
		assert_eq!(Paras::code_by_hash(&old_code.hash()), Some(old_code.clone()));
		assert_eq!(Paras::code_by_hash(&new_code.hash()), Some(new_code.clone()));

		run_to_block(10 + code_retention_period + 1, None);

		// code entry should be pruned now.

		assert_eq!(
			Paras::past_code_meta(&para_id),
			ParaPastCodeMeta { upgrade_times: Vec::new(), last_pruned: Some(10) },
		);

		assert_eq!(Paras::code_by_hash(&old_code.hash()), None); // pruned :(
		assert_eq!(Paras::code_by_hash(&new_code.hash()), Some(new_code.clone()));
	});
}

#[test]
fn code_ref_is_cleaned_correctly() {
	new_test_ext(Default::default()).execute_with(|| {
		let code: ValidationCode = vec![1, 2, 3].into();
		Paras::increase_code_ref(&code.hash(), &code);
		Paras::increase_code_ref(&code.hash(), &code);

		assert!(<Paras as Store>::CodeByHash::contains_key(code.hash()));
		assert_eq!(<Paras as Store>::CodeByHashRefs::get(code.hash()), 2);

		Paras::decrease_code_ref(&code.hash());

		assert!(<Paras as Store>::CodeByHash::contains_key(code.hash()));
		assert_eq!(<Paras as Store>::CodeByHashRefs::get(code.hash()), 1);

		Paras::decrease_code_ref(&code.hash());

		assert!(!<Paras as Store>::CodeByHash::contains_key(code.hash()));
		assert!(!<Paras as Store>::CodeByHashRefs::contains_key(code.hash()));
	});
}

#[test]
fn pvf_check_coalescing_onboarding_and_upgrade() {
	let validation_upgrade_delay = 5;

	let a = ParaId::from(111);
	let b = ParaId::from(222);
	let existing_code: ValidationCode = vec![1, 2, 3].into();
	let validation_code: ValidationCode = vec![3, 2, 1].into();

	let paras = vec![(
		a,
		ParaGenesisArgs {
			parachain: true,
			genesis_head: Default::default(),
			validation_code: existing_code,
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				pvf_checking_enabled: true,
				validation_upgrade_delay,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		// At this point `a` is already onboarded. Run to block 1 performing session change at
		// the end of block #0.
		run_to_block(2, Some(vec![1]));

		// Expected current session index.
		const EXPECTED_SESSION: SessionIndex = 1;
		// Relay parent of the parablock that schedules the upgrade.
		const RELAY_PARENT: BlockNumber = 1;

		// Now we register `b` with `validation_code`
		assert_ok!(Paras::schedule_para_initialize(
			b,
			ParaGenesisArgs {
				parachain: true,
				genesis_head: vec![2].into(),
				validation_code: validation_code.clone(),
			},
		));

		// And now at the same time upgrade `a` to `validation_code`
		Paras::schedule_code_upgrade(
			a,
			validation_code.clone(),
			RELAY_PARENT,
			&Configuration::config(),
		);
		assert!(!Paras::pvfs_require_precheck().is_empty());

		// Supermajority of validators vote for `validation_code`. It should be approved.
		IntoIterator::into_iter([0, 1, 2, 3])
			.map(|i| PvfCheckStatement {
				accept: true,
				subject: validation_code.hash(),
				session_index: EXPECTED_SESSION,
				validator_index: i.into(),
			})
			.for_each(sign_and_include_pvf_check_statement);

		// Check that `b` actually onboards.
		assert_eq!(<Paras as Store>::ActionsQueue::get(EXPECTED_SESSION + 2), vec![b]);

		// Check that the upgrade got scheduled.
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&a),
			Some(RELAY_PARENT + validation_upgrade_delay),
		);

		// Verify that the required events were emitted.
		EventValidator::new()
			.started(&validation_code, b)
			.started(&validation_code, a)
			.accepted(&validation_code, b)
			.accepted(&validation_code, a)
			.check();
	});
}

#[test]
fn pvf_check_onboarding_reject_on_expiry() {
	let pvf_voting_ttl = 2;
	let a = ParaId::from(111);
	let validation_code: ValidationCode = vec![3, 2, 1].into();

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				pvf_checking_enabled: true,
				pvf_voting_ttl,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		run_to_block(1, Some(vec![1]));

		assert_ok!(Paras::schedule_para_initialize(
			a,
			ParaGenesisArgs {
				parachain: false,
				genesis_head: vec![2].into(),
				validation_code: validation_code.clone(),
			},
		));

		// Make sure that we kicked off the PVF vote for this validation code and that the
		// validation code is stored.
		assert!(<Paras as Store>::PvfActiveVoteMap::get(&validation_code.hash()).is_some());
		check_code_is_stored(&validation_code);

		// Skip 2 sessions (i.e. `pvf_voting_ttl`) verifying that the code is still stored in
		// the intermediate session.
		assert_eq!(pvf_voting_ttl, 2);
		run_to_block(2, Some(vec![2]));
		check_code_is_stored(&validation_code);
		run_to_block(3, Some(vec![3]));

		// --- At this point the PVF vote for onboarding should be rejected.

		// Verify that the PVF is no longer stored and there is no active PVF vote.
		check_code_is_not_stored(&validation_code);
		assert!(<Paras as Store>::PvfActiveVoteMap::get(&validation_code.hash()).is_none());
		assert!(Paras::pvfs_require_precheck().is_empty());

		// Verify that at this point we can again try to initialize the same para.
		assert!(Paras::can_schedule_para_initialize(&a));
	});
}

#[test]
fn pvf_check_upgrade_reject() {
	let a = ParaId::from(111);
	let old_code: ValidationCode = vec![1, 2, 3].into();
	let new_code: ValidationCode = vec![3, 2, 1].into();

	let paras = vec![(
		a,
		ParaGenesisArgs {
			parachain: false,
			genesis_head: Default::default(),
			validation_code: old_code,
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { pvf_checking_enabled: true, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		// At this point `a` is already onboarded. Run to block 1 performing session change at
		// the end of block #0.
		run_to_block(2, Some(vec![1]));

		// Relay parent of the block that schedules the upgrade.
		const RELAY_PARENT: BlockNumber = 1;
		// Expected current session index.
		const EXPECTED_SESSION: SessionIndex = 1;

		Paras::schedule_code_upgrade(a, new_code.clone(), RELAY_PARENT, &Configuration::config());
		check_code_is_stored(&new_code);

		// Supermajority of validators vote against `new_code`. PVF should be rejected.
		IntoIterator::into_iter([0, 1, 2, 3])
			.map(|i| PvfCheckStatement {
				accept: false,
				subject: new_code.hash(),
				session_index: EXPECTED_SESSION,
				validator_index: i.into(),
			})
			.for_each(sign_and_include_pvf_check_statement);

		// Verify that the new code is discarded.
		check_code_is_not_stored(&new_code);

		assert!(<Paras as Store>::PvfActiveVoteMap::get(&new_code.hash()).is_none());
		assert!(Paras::pvfs_require_precheck().is_empty());
		assert!(<Paras as Store>::FutureCodeHash::get(&a).is_none());

		// Verify that the required events were emitted.
		EventValidator::new().started(&new_code, a).rejected(&new_code, a).check();
	});
}

#[test]
fn pvf_check_submit_vote_while_disabled() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { pvf_checking_enabled: false, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		// This will set the session index to 1 and seed the validators.
		run_to_block(1, Some(vec![1]));

		let stmt = PvfCheckStatement {
			accept: false,
			subject: ValidationCode(vec![1, 2, 3]).hash(),
			session_index: 1,
			validator_index: 1.into(),
		};

		let signature: ValidatorSignature =
			Sr25519Keyring::Alice.sign(&stmt.signing_payload()).into();

		let call =
			Call::include_pvf_check_statement { stmt: stmt.clone(), signature: signature.clone() };

		let validate_unsigned =
			<Paras as ValidateUnsigned>::validate_unsigned(TransactionSource::InBlock, &call);
		assert_eq!(
			validate_unsigned,
			InvalidTransaction::Custom(INVALID_TX_PVF_CHECK_DISABLED).into()
		);

		assert_err!(
			Paras::include_pvf_check_statement(None.into(), stmt.clone(), signature.clone()),
			Error::<Test>::PvfCheckDisabled
		);
	});
}

#[test]
fn pvf_check_submit_vote() {
	let code_a: ValidationCode = vec![3, 2, 1].into();
	let code_b: ValidationCode = vec![1, 2, 3].into();

	let check = |stmt: PvfCheckStatement| -> (Result<_, _>, Result<_, _>) {
		let validators = &[
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::Eve, // <- this validator is not in the set
		];
		let signature: ValidatorSignature =
			validators[stmt.validator_index.0 as usize].sign(&stmt.signing_payload()).into();

		let call =
			Call::include_pvf_check_statement { stmt: stmt.clone(), signature: signature.clone() };
		let validate_unsigned =
			<Paras as ValidateUnsigned>::validate_unsigned(TransactionSource::InBlock, &call)
				.map(|_| ());
		let dispatch_result =
			Paras::include_pvf_check_statement(None.into(), stmt.clone(), signature.clone())
				.map(|_| ());

		(validate_unsigned, dispatch_result)
	};

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { pvf_checking_enabled: true, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		// Important to run this to seed the validators.
		run_to_block(1, Some(vec![1]));

		assert_ok!(Paras::schedule_para_initialize(
			1000.into(),
			ParaGenesisArgs {
				parachain: false,
				genesis_head: vec![2].into(),
				validation_code: code_a.clone(),
			},
		));

		assert_eq!(
			check(PvfCheckStatement {
				accept: false,
				subject: code_a.hash(),
				session_index: 1,
				validator_index: 1.into(),
			}),
			(Ok(()), Ok(())),
		);

		// A vote in the same direction.
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: false,
			subject: code_a.hash(),
			session_index: 1,
			validator_index: 1.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Custom(INVALID_TX_DOUBLE_VOTE).into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckDoubleVote);

		// Equivocation
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: true,
			subject: code_a.hash(),
			session_index: 1,
			validator_index: 1.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Custom(INVALID_TX_DOUBLE_VOTE).into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckDoubleVote);

		// Vote for an earlier session.
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: false,
			subject: code_a.hash(),
			session_index: 0,
			validator_index: 1.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Stale.into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckStatementStale);

		// Vote for an later session.
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: false,
			subject: code_a.hash(),
			session_index: 2,
			validator_index: 1.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Future.into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckStatementFuture);

		// Validator not in the set.
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: false,
			subject: code_a.hash(),
			session_index: 1,
			validator_index: 5.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Custom(INVALID_TX_BAD_VALIDATOR_IDX).into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckValidatorIndexOutOfBounds);

		// Bad subject (code_b)
		let (unsigned, dispatch) = check(PvfCheckStatement {
			accept: false,
			subject: code_b.hash(),
			session_index: 1,
			validator_index: 1.into(),
		});
		assert_eq!(unsigned, Err(InvalidTransaction::Custom(INVALID_TX_BAD_SUBJECT).into()));
		assert_err!(dispatch, Error::<Test>::PvfCheckSubjectInvalid);
	});
}

#[test]
fn include_pvf_check_statement_refunds_weight() {
	let a = ParaId::from(111);
	let old_code: ValidationCode = vec![1, 2, 3].into();
	let new_code: ValidationCode = vec![3, 2, 1].into();

	let paras = vec![(
		a,
		ParaGenesisArgs {
			parachain: false,
			genesis_head: Default::default(),
			validation_code: old_code,
		},
	)];

	let genesis_config = MockGenesisConfig {
		paras: GenesisConfig { paras, ..Default::default() },
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { pvf_checking_enabled: true, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		// At this point `a` is already onboarded. Run to block 1 performing session change at
		// the end of block #0.
		run_to_block(2, Some(vec![1]));

		// Relay parent of the block that schedules the upgrade.
		const RELAY_PARENT: BlockNumber = 1;
		// Expected current session index.
		const EXPECTED_SESSION: SessionIndex = 1;

		Paras::schedule_code_upgrade(a, new_code.clone(), RELAY_PARENT, &Configuration::config());

		let mut stmts = IntoIterator::into_iter([0, 1, 2, 3])
			.map(|i| {
				let stmt = PvfCheckStatement {
					accept: true,
					subject: new_code.hash(),
					session_index: EXPECTED_SESSION,
					validator_index: (i as u32).into(),
				};
				let sig = VALIDATORS[i].sign(&stmt.signing_payload());
				(stmt, sig)
			})
			.collect::<Vec<_>>();
		let last_one = stmts.pop().unwrap();

		// Verify that just vote submission is priced accordingly.
		for (stmt, sig) in stmts {
			let r = Paras::include_pvf_check_statement(None.into(), stmt, sig.into()).unwrap();
			assert_eq!(r.actual_weight, Some(TestWeightInfo::include_pvf_check_statement()));
		}

		// Verify that the last statement is priced maximally.
		let (stmt, sig) = last_one;
		let r = Paras::include_pvf_check_statement(None.into(), stmt, sig.into()).unwrap();
		assert_eq!(r.actual_weight, None);
	});
}

#[test]
fn add_trusted_validation_code_inserts_with_no_users() {
	// This test is to ensure that trusted validation code is inserted into the storage
	// with the reference count equal to 0.
	let validation_code = ValidationCode(vec![1, 2, 3]);
	new_test_ext(Default::default()).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));
		assert_eq!(<Paras as Store>::CodeByHashRefs::get(&validation_code.hash()), 0,);
	});
}

#[test]
fn add_trusted_validation_code_idempotent() {
	// This test makes sure that calling add_trusted_validation_code twice with the same
	// parameters is a no-op.
	let validation_code = ValidationCode(vec![1, 2, 3]);
	new_test_ext(Default::default()).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));
		assert_storage_noop!({
			assert_ok!(Paras::add_trusted_validation_code(
				RuntimeOrigin::root(),
				validation_code.clone()
			));
		});
	});
}

#[test]
fn poke_unused_validation_code_removes_code_cleanly() {
	// This test makes sure that calling poke_unused_validation_code with a code that is currently
	// in the storage but has no users will remove it cleanly from the storage.
	let validation_code = ValidationCode(vec![1, 2, 3]);
	new_test_ext(Default::default()).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));
		assert_ok!(Paras::poke_unused_validation_code(
			RuntimeOrigin::root(),
			validation_code.hash()
		));

		assert_eq!(<Paras as Store>::CodeByHashRefs::get(&validation_code.hash()), 0);
		assert!(!<Paras as Store>::CodeByHash::contains_key(&validation_code.hash()));
	});
}

#[test]
fn poke_unused_validation_code_doesnt_remove_code_with_users() {
	let para_id = 100.into();
	let validation_code = ValidationCode(vec![1, 2, 3]);
	new_test_ext(Default::default()).execute_with(|| {
		// First we add the code to the storage.
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));

		// Then we add a user to the code, say by upgrading.
		run_to_block(2, None);
		Paras::schedule_code_upgrade(para_id, validation_code.clone(), 1, &Configuration::config());
		Paras::note_new_head(para_id, HeadData::default(), 1);

		// Finally we poke the code, which should not remove it from the storage.
		assert_storage_noop!({
			assert_ok!(Paras::poke_unused_validation_code(
				RuntimeOrigin::root(),
				validation_code.hash()
			));
		});
		check_code_is_stored(&validation_code);
	});
}

#[test]
fn increase_code_ref_doesnt_have_allergy_on_add_trusted_validation_code() {
	// Verify that accidential calling of increase_code_ref or decrease_code_ref does not lead
	// to a disaster.
	// NOTE that this test is extra paranoid, as it is not really possible to hit
	// `decrease_code_ref` without calling `increase_code_ref` first.
	let code = ValidationCode(vec![1, 2, 3]);

	new_test_ext(Default::default()).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(RuntimeOrigin::root(), code.clone()));
		Paras::increase_code_ref(&code.hash(), &code);
		Paras::increase_code_ref(&code.hash(), &code);
		assert!(<Paras as Store>::CodeByHash::contains_key(code.hash()));
		assert_eq!(<Paras as Store>::CodeByHashRefs::get(code.hash()), 2);
	});

	new_test_ext(Default::default()).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(RuntimeOrigin::root(), code.clone()));
		Paras::decrease_code_ref(&code.hash());
		assert!(<Paras as Store>::CodeByHash::contains_key(code.hash()));
		assert_eq!(<Paras as Store>::CodeByHashRefs::get(code.hash()), 0);
	});
}

#[test]
fn add_trusted_validation_code_insta_approval() {
	// In particular, this tests that `kick_off_pvf_check` reacts to the `add_trusted_validation_code`
	// and uses the `CodeByHash::contains_key` which is what `add_trusted_validation_code` uses.
	let para_id = 100.into();
	let validation_code = ValidationCode(vec![1, 2, 3]);
	let validation_upgrade_delay = 25;
	let minimum_validation_upgrade_delay = 2;
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				pvf_checking_enabled: true,
				validation_upgrade_delay,
				minimum_validation_upgrade_delay,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};
	new_test_ext(genesis_config).execute_with(|| {
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));

		// Then some parachain upgrades it's code with the relay-parent 1.
		run_to_block(2, None);
		Paras::schedule_code_upgrade(para_id, validation_code.clone(), 1, &Configuration::config());
		Paras::note_new_head(para_id, HeadData::default(), 1);

		// Verify that the code upgrade has `expected_at` set to `26`. This is the behavior
		// equal to that of `pvf_checking_enabled: false`.
		assert_eq!(
			<Paras as Store>::FutureCodeUpgrades::get(&para_id),
			Some(1 + validation_upgrade_delay)
		);

		// Verify that the required events were emitted.
		EventValidator::new()
			.started(&validation_code, para_id)
			.accepted(&validation_code, para_id)
			.check();
	});
}

#[test]
fn add_trusted_validation_code_enacts_existing_pvf_vote() {
	// This test makes sure that calling `add_trusted_validation_code` with a code that is
	// already going through PVF pre-checking voting will conclude the voting and enact the
	// code upgrade.
	let para_id = 100.into();
	let validation_code = ValidationCode(vec![1, 2, 3]);
	let validation_upgrade_delay = 25;
	let minimum_validation_upgrade_delay = 2;
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				pvf_checking_enabled: true,
				validation_upgrade_delay,
				minimum_validation_upgrade_delay,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};
	new_test_ext(genesis_config).execute_with(|| {
		// First, some parachain upgrades it's code with the relay-parent 1.
		run_to_block(2, None);
		Paras::schedule_code_upgrade(para_id, validation_code.clone(), 1, &Configuration::config());
		Paras::note_new_head(para_id, HeadData::default(), 1);

		// No upgrade should be scheduled at this point. PVF pre-checking vote should run for
		// that PVF.
		assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
		assert!(<Paras as Store>::PvfActiveVoteMap::contains_key(&validation_code.hash()));

		// Then we add a trusted validation code. That should conclude the vote.
		assert_ok!(Paras::add_trusted_validation_code(
			RuntimeOrigin::root(),
			validation_code.clone()
		));
		assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_some());
		assert!(!<Paras as Store>::PvfActiveVoteMap::contains_key(&validation_code.hash()));
	});
}

#[test]
fn verify_upgrade_go_ahead_signal_is_externally_accessible() {
	use primitives::v2::well_known_keys;

	let a = ParaId::from(2020);

	new_test_ext(Default::default()).execute_with(|| {
		assert!(sp_io::storage::get(&well_known_keys::upgrade_go_ahead_signal(a)).is_none());
		<Paras as Store>::UpgradeGoAheadSignal::insert(&a, UpgradeGoAhead::GoAhead);
		assert_eq!(
			sp_io::storage::get(&well_known_keys::upgrade_go_ahead_signal(a)).unwrap(),
			vec![1u8],
		);
	});
}

#[test]
fn verify_upgrade_restriction_signal_is_externally_accessible() {
	use primitives::v2::well_known_keys;

	let a = ParaId::from(2020);

	new_test_ext(Default::default()).execute_with(|| {
		assert!(sp_io::storage::get(&well_known_keys::upgrade_restriction_signal(a)).is_none());
		<Paras as Store>::UpgradeRestrictionSignal::insert(&a, UpgradeRestriction::Present);
		assert_eq!(
			sp_io::storage::get(&well_known_keys::upgrade_restriction_signal(a)).unwrap(),
			vec![0],
		);
	});
}
