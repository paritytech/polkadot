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
use crate::{inclusion, ParaId};
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use sp_std::collections::btree_map::BTreeMap;

use crate::builder::BenchBuilder;

benchmarks! {
	// Variant over `v`, the number of dispute statements in a dispute statement set. This gives the
	// weight of a single dispute statement set.
	enter_variable_disputes {
		let v in 10..BenchBuilder::<T>::fallback_max_validators();

		let scenario = BenchBuilder::<T>::new()
			.set_dispute_sessions(&[2])
			.build();

		let mut benchmark = scenario.data.clone();
		let dispute = benchmark.disputes.pop().unwrap();

		benchmark.bitfields.clear();
		benchmark.backed_candidates.clear();
		benchmark.disputes.clear();

		benchmark.disputes.push(dispute);
		benchmark.disputes.get_mut(0).unwrap().statements.drain(v as usize..);
	}: enter(RawOrigin::None, benchmark)
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());

		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario._session);
	}

	// The weight of one bitfield.
	enter_bitfields {
		let cores_with_backed: BTreeMap<_, _>
			= vec![(0, BenchBuilder::<T>::fallback_max_validators())]
				.into_iter()
				.collect();

		let scenario = BenchBuilder::<T>::new()
			.set_backed_and_concluding_cores(cores_with_backed)
			.build();

		let mut benchmark = scenario.data.clone();
		let bitfield = benchmark.bitfields.pop().unwrap();

		benchmark.bitfields.clear();
		benchmark.backed_candidates.clear();
		benchmark.disputes.clear();

		benchmark.bitfields.push(bitfield);
	}: enter(RawOrigin::None, benchmark)
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();
		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario._session);
	}

	// Variant over `v`, the amount of validity votes for a backed candidate. This gives the weight
	// of a single backed candidate.
	enter_backed_candidates_variable {
		// NOTE: the starting value must be over half of the max validators per group so the backed
		// candidate is not rejected. Also, we cannot have more validity votes than validators in
		// the group.

		// Do not use this range for Rococo because it only has 1 validator per backing group,
		// which causes issues when trying to create slopes with the benchmarking analysis. Instead
		// use v = 1 for running Rococo benchmarks
		let v in (BenchBuilder::<T>::fallback_min_validity_votes())
			..(BenchBuilder::<T>::fallback_max_validators());

		// Comment in for running rococo benchmarks
		// let v = 1;

		let cores_with_backed: BTreeMap<_, _>
			= vec![(0, v)] // The backed candidate will have `v` validity votes.
				.into_iter()
				.collect();

		let scenario = BenchBuilder::<T>::new()
			.set_backed_and_concluding_cores(cores_with_backed.clone())
			.build();

		let mut benchmark = scenario.data.clone();

		// There is 1 backed,
		assert_eq!(benchmark.backed_candidates.len(), 1);
		// with `v` validity votes.
		assert_eq!(benchmark.backed_candidates.get(0).unwrap().validity_votes.len(), v as usize);

		benchmark.bitfields.clear();
		benchmark.disputes.clear();
	}: enter(RawOrigin::None, benchmark)
	verify {
		let max_validators_per_core = BenchBuilder::<T>::fallback_max_validators_per_core();
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();
		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario._session);
		// Ensure that there are an expected number of candidates
		let header = BenchBuilder::<T>::header(scenario._block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), v as usize);
		}

		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			cores_with_backed.len()
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			cores_with_backed.len()
		);
	}

	enter_backed_candidate_code_upgrade {
		// For now we always assume worst case code size. In the future we could vary over this.
		let v = crate::configuration::Pallet::<T>::config().max_code_size;

		let cores_with_backed: BTreeMap<_, _>
			= vec![(0, BenchBuilder::<T>::fallback_min_validity_votes())]
				.into_iter()
				.collect();

		let scenario = BenchBuilder::<T>::new()
			.set_backed_and_concluding_cores(cores_with_backed.clone())
			.set_code_upgrade(v)
			.build();

		let mut benchmark = scenario.data.clone();

		// There is 1 backed
		assert_eq!(benchmark.backed_candidates.len(), 1);
		assert_eq!(
			benchmark.backed_candidates.get(0).unwrap().validity_votes.len() as u32,
			BenchBuilder::<T>::fallback_min_validity_votes()
		);

		benchmark.bitfields.clear();
		benchmark.disputes.clear();
		crate::paras::benchmarking::generate_disordered_upgrades::<T>();
	}: enter(RawOrigin::None, benchmark)
	verify {
		let max_validators_per_core = BenchBuilder::<T>::fallback_max_validators_per_core();
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();
		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario._session);
		// Ensure that there are an expected number of candidates
		let header = BenchBuilder::<T>::header(scenario._block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators)
			in vote.backing_validators_per_candidate.iter().enumerate() {
				let descriptor = backing_validators.0.descriptor();
				assert_eq!(ParaId::from(para_id), descriptor.para_id);
				assert_eq!(header.hash(), descriptor.relay_parent);
				assert_eq!(
					backing_validators.1.len() as u32,
					BenchBuilder::<T>::fallback_min_validity_votes()
				);
			}

		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			cores_with_backed.len()
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			cores_with_backed.len()
		);
	}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
