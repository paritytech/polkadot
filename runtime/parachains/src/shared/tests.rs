// Copyright (C) Parity Technologies (UK) Ltd.
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
use assert_matches::assert_matches;
use keyring::Sr25519Keyring;
use primitives::Hash;

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

#[test]
fn tracker_earliest_block_number() {
	let mut tracker = AllowedRelayParentsTracker::default();

	// Test it on an empty tracker.
	let now: u32 = 1;
	let max_ancestry_len = 5;
	assert_eq!(tracker.hypothetical_earliest_block_number(now, max_ancestry_len), now);

	// Push a single block into the tracker, suppose max capacity is 1.
	let max_ancestry_len = 0;
	tracker.update(Hash::zero(), Hash::zero(), 0, max_ancestry_len);
	assert_eq!(tracker.hypothetical_earliest_block_number(now, max_ancestry_len), now);

	// Test a greater capacity.
	let max_ancestry_len = 4;
	let now = 4;
	for i in 1..now {
		tracker.update(Hash::zero(), Hash::zero(), i, max_ancestry_len);
		assert_eq!(tracker.hypothetical_earliest_block_number(i + 1, max_ancestry_len), 0);
	}

	// Capacity exceeded.
	tracker.update(Hash::zero(), Hash::zero(), now, max_ancestry_len);
	assert_eq!(tracker.hypothetical_earliest_block_number(now + 1, max_ancestry_len), 1);
}

#[test]
fn tracker_acquire_info() {
	let mut tracker = AllowedRelayParentsTracker::<Hash, u32>::default();
	let max_ancestry_len = 2;

	// (relay_parent, state_root) pairs.
	let blocks = &[
		(Hash::repeat_byte(0), Hash::repeat_byte(10)),
		(Hash::repeat_byte(1), Hash::repeat_byte(11)),
		(Hash::repeat_byte(2), Hash::repeat_byte(12)),
	];

	let (relay_parent, state_root) = blocks[0];
	tracker.update(relay_parent, state_root, 0, max_ancestry_len);
	assert_matches!(
		tracker.acquire_info(relay_parent, None),
		Some((s, b)) if s == state_root && b == 0
	);

	let (relay_parent, state_root) = blocks[1];
	tracker.update(relay_parent, state_root, 1u32, max_ancestry_len);
	let (relay_parent, state_root) = blocks[2];
	tracker.update(relay_parent, state_root, 2u32, max_ancestry_len);
	for (block_num, (rp, state_root)) in blocks.iter().enumerate().take(2) {
		assert_matches!(
			tracker.acquire_info(*rp, None),
			Some((s, b)) if &s == state_root && b == block_num as u32
		);

		assert!(tracker.acquire_info(*rp, Some(2)).is_none());
	}

	for (block_num, (rp, state_root)) in blocks.iter().enumerate().skip(1) {
		assert_matches!(
			tracker.acquire_info(*rp, Some(block_num as u32 - 1)),
			Some((s, b)) if &s == state_root && b == block_num as u32
		);
	}
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
