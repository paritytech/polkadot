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

// In order to facilitate benchmarks as tests we have a benchmark feature gated `WeightInfo` impl
// that uses 0 for all the weights. Because all the weights are 0, the tests that rely on
// weights for limiting data will fail, so we don't run them when using the benchmark feature.
#[cfg(not(feature = "runtime-benchmarks"))]
mod enter {
	use super::*;
	use crate::{
		builder::{Bench, BenchBuilder},
		mock::{new_test_ext, MockGenesisConfig, Test},
	};
	use frame_support::assert_ok;
	use sp_std::collections::btree_map::BTreeMap;

	struct TestConfig {
		dispute_statements: BTreeMap<u32, u32>,
		dispute_sessions: Vec<u32>,
		backed_and_concluding: BTreeMap<u32, u32>,
		num_validators_per_core: u32,
		code_upgrade: Option<u32>,
	}

	fn make_inherent_data(
		TestConfig {
			dispute_statements,
			dispute_sessions,
			backed_and_concluding,
			num_validators_per_core,
			code_upgrade,
		}: TestConfig,
	) -> Bench<Test> {
		let builder = BenchBuilder::<Test>::new()
			.set_max_validators(
				(dispute_sessions.len() + backed_and_concluding.len()) as u32 *
					num_validators_per_core,
			)
			.set_max_validators_per_core(num_validators_per_core)
			.set_dispute_statements(dispute_statements)
			.set_backed_and_concluding_cores(backed_and_concluding)
			.set_dispute_sessions(&dispute_sessions[..]);

		if let Some(code_size) = code_upgrade {
			builder.set_code_upgrade(code_size).build()
		} else {
			builder.build()
		}
	}

	#[test]
	// Validate that if we create 2 backed candidates which are assigned to 2 cores that will be freed via
	// becoming fully available, the backed candidates will not be filtered out in `create_inherent` and
	// will not cause `enter` to early.
	fn include_backed_candidates() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let dispute_statements = BTreeMap::new();

			let mut backed_and_concluding = BTreeMap::new();
			backed_and_concluding.insert(0, 1);
			backed_and_concluding.insert(1, 1);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![], // No disputes
				backed_and_concluding,
				num_validators_per_core: 1,
				code_upgrade: None,
			});

			// We expect the scenario to have cores 0 & 1 with pending availability. The backed
			// candidates are also created for cores 0 & 1, so once the pending available
			// become fully available those cores are marked as free and scheduled for the backed
			// candidates.
			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (2 validators)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 2);
			// * 1 backed candidate per core (2 cores)
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 0 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 0);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			// Nothing is filtered out (including the backed candidates.)
			assert_eq!(
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap(),
				expected_para_inherent_data
			);

			// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
			// alter storage, but just double checking for sanity).
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_eq!(Pallet::<Test>::on_chain_votes(), None);
			// Call enter with our 2 backed candidates
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				expected_para_inherent_data
			));
			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know our 2
				// backed candidates did not get filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				2
			);
		});
	}

	#[test]
	// Ensure that disputes are filtered out if the session is in the future.
	fn filter_multi_dispute_data() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let dispute_statements = BTreeMap::new();

			let backed_and_concluding = BTreeMap::new();

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![1, 2, 3 /* Session 3 too new, will get filtered out */],
				backed_and_concluding,
				num_validators_per_core: 5,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (5 validators per core, 3 disputes => 3 cores, 15 validators)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 15);
			// * 0 backed candidate per core
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			let multi_dispute_inherent_data =
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
			// Dispute for session that lies too far in the future should be filtered out
			assert!(multi_dispute_inherent_data != expected_para_inherent_data);

			assert_eq!(multi_dispute_inherent_data.disputes.len(), 2);

			// Assert that the first 2 disputes are included
			assert_eq!(
				&multi_dispute_inherent_data.disputes[..2],
				&expected_para_inherent_data.disputes[..2],
			);

			// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
			// alter storage, but just double checking for sanity).
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_eq!(Pallet::<Test>::on_chain_votes(), None);
			// Call enter with our 2 disputes
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				multi_dispute_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know there
				// where no backed candidates included
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0
			);
		});
	}

	#[test]
	// Ensure that when dispute data establishes an over weight block that we adequately
	// filter out disputes according to our prioritization rule
	fn limit_dispute_data() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let dispute_statements = BTreeMap::new();
			// No backed and concluding cores, so all cores will be filled with disputes.
			let backed_and_concluding = BTreeMap::new();

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 6,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (6 validators per core, 3 disputes => 18 validators)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 18);
			// * 0 backed candidate per core
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			let limit_inherent_data =
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
			// Expect that inherent data is filtered to include only 2 disputes
			assert!(limit_inherent_data != expected_para_inherent_data);

			// Ensure that the included disputes are sorted by session
			assert_eq!(limit_inherent_data.disputes.len(), 2);
			assert_eq!(limit_inherent_data.disputes[0].session, 1);
			assert_eq!(limit_inherent_data.disputes[1].session, 2);

			// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
			// alter storage, but just double checking for sanity).
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_eq!(Pallet::<Test>::on_chain_votes(), None);
			// Call enter with our 2 disputes
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				limit_inherent_data,
			));

			assert_eq!(
				// Ensure that our inherent data did not included backed candidates as expected
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0
			);
		});
	}

	#[test]
	// Ensure that when dispute data establishes an over weight block that we abort
	// due to an over weight block
	fn limit_dispute_data_overweight() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let dispute_statements = BTreeMap::new();
			// No backed and concluding cores, so all cores will be filled with disputes.
			let backed_and_concluding = BTreeMap::new();

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 6,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (6 validators per core, 3 disputes => 18 validators)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 18);
			// * 0 backed candidate per core
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				expected_para_inherent_data,
			));
		});
	}

	#[test]
	// Ensure that when a block is over weight due to disputes, but there is still sufficient
	// block weight to include a number of signed bitfields, the inherent data is filtered
	// as expected
	fn limit_dispute_data_ignore_backed_candidates() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let dispute_statements = BTreeMap::new();

			let mut backed_and_concluding = BTreeMap::new();
			// 2 backed candidates shall be scheduled
			backed_and_concluding.insert(0, 2);
			backed_and_concluding.insert(1, 2);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 4,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (4 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 20);
			// * 2 backed candidates
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			// Nothing is filtered out (including the backed candidates.)
			let limit_inherent_data =
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
			assert!(limit_inherent_data != expected_para_inherent_data);

			// Three disputes is over weight (see previous test), so we expect to only see 2 disputes
			assert_eq!(limit_inherent_data.disputes.len(), 2);
			// Ensure disputes are filtered as expected
			assert_eq!(limit_inherent_data.disputes[0].session, 1);
			assert_eq!(limit_inherent_data.disputes[1].session, 2);
			// Ensure all bitfields are included as these are still not over weight
			assert_eq!(
				limit_inherent_data.bitfields.len(),
				expected_para_inherent_data.bitfields.len()
			);
			// Ensure that all backed candidates are filtered out as either would make the block over weight
			assert_eq!(limit_inherent_data.backed_candidates.len(), 0);

			// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
			// alter storage, but just double checking for sanity).
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_eq!(Pallet::<Test>::on_chain_votes(), None);
			// Call enter with our 2 disputes
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				limit_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know
				// all of our candidates got filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0,
			);
		});
	}

	#[test]
	// Ensure that we abort if we encounter an over weight block for disputes + bitfields
	fn limit_dispute_data_ignore_backed_candidates_overweight() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let dispute_statements = BTreeMap::new();

			let mut backed_and_concluding = BTreeMap::new();
			// 2 backed candidates shall be scheduled
			backed_and_concluding.insert(0, 2);
			backed_and_concluding.insert(1, 2);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 4,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (4 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 20);
			// * 2 backed candidates
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			// Ensure that calling enter with 3 disputes and 2 candidates is over weight
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				expected_para_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know
				// all of our candidates got filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0,
			);
		});
	}

	#[test]
	// Ensure that when a block is over weight due to disputes and bitfields, the bitfields are
	// filtered to accommodate the block size and no backed candidates are included.
	fn limit_bitfields() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let mut dispute_statements = BTreeMap::new();
			// Cap the number of statements per dispute to 20 in order to ensure we have enough
			// space in the block for some (but not all) bitfields
			dispute_statements.insert(2, 20);
			dispute_statements.insert(3, 20);
			dispute_statements.insert(4, 20);

			let mut backed_and_concluding = BTreeMap::new();
			// Schedule 2 backed candidates
			backed_and_concluding.insert(0, 2);
			backed_and_concluding.insert(1, 2);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 5,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20),
			assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
			// * 2 backed candidates,
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			// Nothing is filtered out (including the backed candidates.)
			let limit_inherent_data =
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
			assert!(limit_inherent_data != expected_para_inherent_data);

			// Three disputes is over weight (see previous test), so we expect to only see 2 disputes
			assert_eq!(limit_inherent_data.disputes.len(), 2);
			// Ensure disputes are filtered as expected
			assert_eq!(limit_inherent_data.disputes[0].session, 1);
			assert_eq!(limit_inherent_data.disputes[1].session, 2);
			// Ensure all bitfields are included as these are still not over weight
			assert_eq!(limit_inherent_data.bitfields.len(), 20,);
			// Ensure that all backed candidates are filtered out as either would make the block over weight
			assert_eq!(limit_inherent_data.backed_candidates.len(), 0);

			// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
			// alter storage, but just double checking for sanity).
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_eq!(Pallet::<Test>::on_chain_votes(), None);
			// Call enter with our 2 disputes
			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				limit_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know
				// all of our candidates got filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0,
			);
		});
	}

	#[test]
	// Ensure that when a block is over weight due to disputes and bitfields, we abort
	fn limit_bitfields_overweight() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let mut dispute_statements = BTreeMap::new();
			// Control the number of statements per dispute to ensure we have enough space
			// in the block for some (but not all) bitfields
			dispute_statements.insert(2, 20);
			dispute_statements.insert(3, 20);
			dispute_statements.insert(4, 20);

			let mut backed_and_concluding = BTreeMap::new();
			// 2 backed candidates shall be scheduled
			backed_and_concluding.insert(0, 2);
			backed_and_concluding.insert(1, 2);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 5,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
			// * 2 backed candidates
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				expected_para_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know
				// all of our candidates got filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0,
			);
		});
	}

	#[test]
	// Ensure that when a block is over weight due to disputes and bitfields, we abort
	fn limit_candidates_over_weight_1() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let mut dispute_statements = BTreeMap::new();
			// Control the number of statements per dispute to ensure we have enough space
			// in the block for some (but not all) bitfields
			dispute_statements.insert(2, 17);
			dispute_statements.insert(3, 17);
			dispute_statements.insert(4, 17);

			let mut backed_and_concluding = BTreeMap::new();
			// 2 backed candidates shall be scheduled
			backed_and_concluding.insert(0, 16);
			backed_and_concluding.insert(1, 25);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 5,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
			// * 2 backed candidates
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);
			let mut inherent_data = InherentData::new();
			inherent_data
				.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
				.unwrap();

			let limit_inherent_data =
				Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
			// Expect that inherent data is filtered to include only 1 backed candidate and 2 disputes
			assert!(limit_inherent_data != expected_para_inherent_data);

			// * 1 bitfields
			assert_eq!(limit_inherent_data.bitfields.len(), 25);
			// * 2 backed candidates
			assert_eq!(limit_inherent_data.backed_candidates.len(), 1);
			// * 3 disputes.
			assert_eq!(limit_inherent_data.disputes.len(), 2);

			// The current schedule is empty prior to calling `create_inherent_enter`.
			assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				limit_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know our 2
				// backed candidates did not get filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				1
			);
		});
	}

	#[test]
	// Ensure that when a block is over weight due to disputes and bitfields, we abort
	fn limit_candidates_over_weight_0() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			// Create the inherent data for this block
			let mut dispute_statements = BTreeMap::new();
			// Control the number of statements per dispute to ensure we have enough space
			// in the block for some (but not all) bitfields
			dispute_statements.insert(2, 17);
			dispute_statements.insert(3, 17);
			dispute_statements.insert(4, 17);

			let mut backed_and_concluding = BTreeMap::new();
			// 2 backed candidates shall be scheduled
			backed_and_concluding.insert(0, 16);
			backed_and_concluding.insert(1, 25);

			let scenario = make_inherent_data(TestConfig {
				dispute_statements,
				dispute_sessions: vec![2, 2, 1], // 3 cores with disputes
				backed_and_concluding,
				num_validators_per_core: 5,
				code_upgrade: None,
			});

			let expected_para_inherent_data = scenario.data.clone();

			// Check the para inherent data is as expected:
			// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
			assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
			// * 2 backed candidates
			assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
			// * 3 disputes.
			assert_eq!(expected_para_inherent_data.disputes.len(), 3);

			assert_ok!(Pallet::<Test>::enter(
				frame_system::RawOrigin::None.into(),
				expected_para_inherent_data,
			));

			assert_eq!(
				// The length of this vec is equal to the number of candidates, so we know our 2
				// backed candidates did not get filtered out
				Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
				0
			);
		});
	}
}

fn default_header() -> primitives::v1::Header {
	primitives::v1::Header {
		parent_hash: Default::default(),
		number: 0,
		state_root: Default::default(),
		extrinsics_root: Default::default(),
		digest: Default::default(),
	}
}

mod sanitizers {
	use super::*;

	use crate::inclusion::tests::{
		back_candidate, collator_sign_candidate, BackingKind, TestCandidateBuilder,
	};
	use bitvec::order::Lsb0;
	use primitives::v1::{
		AvailabilityBitfield, GroupIndex, Hash, Id as ParaId, SignedAvailabilityBitfield,
		ValidatorIndex,
	};
	use sp_core::crypto::UncheckedFrom;

	use crate::mock::Test;
	use futures::executor::block_on;
	use keyring::Sr25519Keyring;
	use primitives::v0::PARACHAIN_KEY_TYPE_ID;
	use sc_keystore::LocalKeystore;
	use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
	use std::sync::Arc;

	fn validator_pubkeys(val_ids: &[keyring::Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	#[test]
	fn bitfields() {
		let header = default_header();
		let parent_hash = header.hash();
		// 2 cores means two bits
		let expected_bits = 2;
		let session_index = SessionIndex::from(0_u32);

		let crypto_store = LocalKeystore::in_memory();
		let crypto_store = Arc::new(crypto_store) as SyncCryptoStorePtr;
		let signing_context = SigningContext { parent_hash, session_index };

		let validators = vec![
			keyring::Sr25519Keyring::Alice,
			keyring::Sr25519Keyring::Bob,
			keyring::Sr25519Keyring::Charlie,
			keyring::Sr25519Keyring::Dave,
		];
		for validator in validators.iter() {
			SyncCryptoStore::sr25519_generate_new(
				&*crypto_store,
				PARACHAIN_KEY_TYPE_ID,
				Some(&validator.to_seed()),
			)
			.unwrap();
		}
		let validator_public = validator_pubkeys(&validators);

		let unchecked_bitfields = [
			BitVec::<Lsb0, u8>::repeat(true, expected_bits),
			BitVec::<Lsb0, u8>::repeat(true, expected_bits),
			{
				let mut bv = BitVec::<Lsb0, u8>::repeat(false, expected_bits);
				bv.set(expected_bits - 1, true);
				bv
			},
		]
		.iter()
		.enumerate()
		.map(|(vi, ab)| {
			let validator_index = ValidatorIndex::from(vi as u32);
			block_on(SignedAvailabilityBitfield::sign(
				&crypto_store,
				AvailabilityBitfield::from(ab.clone()),
				&signing_context,
				validator_index,
				&validator_public[vi],
			))
			.unwrap()
			.unwrap()
			.into_unchecked()
		})
		.collect::<Vec<_>>();

		let disputed_bitfield = DisputedBitfield::zeros(expected_bits);

		{
			assert_eq!(
				sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Skip,
				),
				unchecked_bitfields.clone()
			);
			assert_eq!(
				sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Yes
				),
				unchecked_bitfields.clone()
			);
		}

		// disputed bitfield is non-zero
		{
			let mut disputed_bitfield = DisputedBitfield::zeros(expected_bits);
			// pretend the first core was freed by either a malicious validator
			// or by resolved dispute
			disputed_bitfield.0.set(0, true);

			assert_eq!(
				sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Yes
				)
				.len(),
				1
			);
			assert_eq!(
				sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Skip
				)
				.len(),
				1
			);
		}

		// bitfield size mismatch
		{
			assert!(sanitize_bitfields::<Test>(
				unchecked_bitfields.clone(),
				disputed_bitfield.clone(),
				expected_bits + 1,
				parent_hash,
				session_index,
				&validator_public[..],
				FullCheck::Yes
			)
			.is_empty());
			assert!(sanitize_bitfields::<Test>(
				unchecked_bitfields.clone(),
				disputed_bitfield.clone(),
				expected_bits + 1,
				parent_hash,
				session_index,
				&validator_public[..],
				FullCheck::Skip
			)
			.is_empty());
		}

		// remove the last validator
		{
			let shortened = validator_public.len() - 2;
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..shortened],
					FullCheck::Yes,
				)[..],
				&unchecked_bitfields[..shortened]
			);
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..shortened],
					FullCheck::Skip,
				)[..],
				&unchecked_bitfields[..shortened]
			);
		}

		// switch ordering of bitfields
		{
			let mut unchecked_bitfields = unchecked_bitfields.clone();
			let x = unchecked_bitfields.swap_remove(0);
			unchecked_bitfields.push(x);
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Yes
				)[..],
				&unchecked_bitfields[..(unchecked_bitfields.len() - 2)]
			);
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Skip
				)[..],
				&unchecked_bitfields[..(unchecked_bitfields.len() - 2)]
			);
		}

		// check the validators signature
		{
			let mut unchecked_bitfields = unchecked_bitfields.clone();

			// insert a bad signature for the last bitfield
			let last_bit_idx = unchecked_bitfields.len() - 1;
			unchecked_bitfields
				.get_mut(last_bit_idx)
				.and_then(|u| Some(u.set_signature(UncheckedFrom::unchecked_from([1u8; 64]))))
				.expect("we are accessing a valid index");
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Yes
				)[..],
				&unchecked_bitfields[..last_bit_idx]
			);
			assert_eq!(
				&sanitize_bitfields::<Test>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits,
					parent_hash,
					session_index,
					&validator_public[..],
					FullCheck::Skip
				)[..],
				&unchecked_bitfields[..]
			);
		}
	}

	#[test]
	fn candidates() {
		const RELAY_PARENT_NUM: u32 = 3;

		let header = default_header();
		let relay_parent = header.hash();
		let session_index = SessionIndex::from(0_u32);

		let keystore = LocalKeystore::in_memory();
		let keystore = Arc::new(keystore) as SyncCryptoStorePtr;
		let signing_context = SigningContext { parent_hash: relay_parent, session_index };

		let validators = vec![
			keyring::Sr25519Keyring::Alice,
			keyring::Sr25519Keyring::Bob,
			keyring::Sr25519Keyring::Charlie,
			keyring::Sr25519Keyring::Dave,
		];
		for validator in validators.iter() {
			SyncCryptoStore::sr25519_generate_new(
				&*keystore,
				PARACHAIN_KEY_TYPE_ID,
				Some(&validator.to_seed()),
			)
			.unwrap();
		}

		let has_concluded_invalid =
			|_idx: usize, _backed_candidate: &BackedCandidate| -> bool { false };

		let scheduled = (0_usize..2)
			.into_iter()
			.map(|idx| {
				let ca = CoreAssignment {
					kind: scheduler::AssignmentKind::Parachain,
					group_idx: GroupIndex::from(idx as u32),
					para_id: ParaId::from(1_u32 + idx as u32),
					core: CoreIndex::from(idx as u32),
				};
				ca
			})
			.collect::<Vec<_>>();
		let scheduled = &scheduled[..];

		let group_validators = |group_index: GroupIndex| {
			match group_index {
				group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
				group_index if group_index == GroupIndex::from(1) => Some(vec![2, 3]),
				_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
			}
			.map(|m| m.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
		};

		let backed_candidates = (0_usize..2)
			.into_iter()
			.map(|idx0| {
				let idx1 = idx0 + 1;
				let mut candidate = TestCandidateBuilder {
					para_id: ParaId::from(idx1),
					relay_parent,
					pov_hash: Hash::repeat_byte(idx1 as u8),
					persisted_validation_data_hash: [42u8; 32].into(),
					hrmp_watermark: RELAY_PARENT_NUM,
					..Default::default()
				}
				.build();

				collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

				let backed = block_on(back_candidate(
					candidate,
					&validators,
					group_validators(GroupIndex::from(idx0 as u32)).unwrap().as_ref(),
					&keystore,
					&signing_context,
					BackingKind::Threshold,
				));
				backed
			})
			.collect::<Vec<_>>();

		// happy path
		assert_eq!(
			sanitize_backed_candidates::<Test, _>(
				relay_parent,
				backed_candidates.clone(),
				has_concluded_invalid,
				scheduled
			),
			backed_candidates
		);

		// nothing is scheduled, so no paraids match, thus all backed candidates are skipped
		{
			let scheduled = &[][..];
			assert!(sanitize_backed_candidates::<Test, _>(
				relay_parent,
				backed_candidates.clone(),
				has_concluded_invalid,
				scheduled
			)
			.is_empty());
		}

		// relay parent mismatch
		{
			let relay_parent = Hash::repeat_byte(0xFA);
			assert!(sanitize_backed_candidates::<Test, _>(
				relay_parent,
				backed_candidates.clone(),
				has_concluded_invalid,
				scheduled
			)
			.is_empty());
		}

		// candidates that have concluded as invalid are filtered out
		{
			// mark every second one as concluded invalid
			let set = {
				let mut set = std::collections::HashSet::new();
				for (idx, backed_candidate) in backed_candidates.iter().enumerate() {
					if idx & 0x01 == 0 {
						set.insert(backed_candidate.hash().clone());
					}
				}
				set
			};
			let has_concluded_invalid =
				|_idx: usize, candidate: &BackedCandidate| set.contains(&candidate.hash());
			assert_eq!(
				sanitize_backed_candidates::<Test, _>(
					relay_parent,
					backed_candidates.clone(),
					has_concluded_invalid,
					scheduled
				)
				.len(),
				backed_candidates.len() / 2
			);
		}
	}
}
