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
	mock::{new_test_ext, MockGenesisConfig, ParasShared},
};
use keyring::Sr25519Keyring;

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

#[test]
fn sets_and_shuffles_validators() {
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];

	let mut config = HostConfiguration::default();
	config.max_validators = None;

	let pubkeys = validator_pubkeys(&validators);

	new_test_ext(MockGenesisConfig::default()).execute_with(|| {
		let validators = ParasShared::initializer_on_new_session(1, [1; 32], &config, pubkeys);

		assert_eq!(
			validators,
			validator_pubkeys(&[
				Sr25519Keyring::Ferdie,
				Sr25519Keyring::Bob,
				Sr25519Keyring::Charlie,
				Sr25519Keyring::Dave,
				Sr25519Keyring::Alice,
			])
		);

		assert_eq!(ParasShared::active_validator_keys(), validators);

		assert_eq!(
			ParasShared::active_validator_indices(),
			vec![
				ValidatorIndex(4),
				ValidatorIndex(1),
				ValidatorIndex(2),
				ValidatorIndex(3),
				ValidatorIndex(0),
			]
		);
	});
}

#[test]
fn sets_truncates_and_shuffles_validators() {
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];

	let mut config = HostConfiguration::default();
	config.max_validators = Some(2);

	let pubkeys = validator_pubkeys(&validators);

	new_test_ext(MockGenesisConfig::default()).execute_with(|| {
		let validators = ParasShared::initializer_on_new_session(1, [1; 32], &config, pubkeys);

		assert_eq!(validators, validator_pubkeys(&[Sr25519Keyring::Ferdie, Sr25519Keyring::Bob,]));

		assert_eq!(ParasShared::active_validator_keys(), validators);

		assert_eq!(
			ParasShared::active_validator_indices(),
			vec![ValidatorIndex(4), ValidatorIndex(1),]
		);
	});
}
