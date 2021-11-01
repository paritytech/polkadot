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
use crate::{inclusion, ParaId};
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;

use crate::builder::BenchBuilder;

benchmarks! {
	// Variant over `v`, the number of dispute statements in a dispute statement set. This gives the
	// weight of a single dispute statement set.
	enter_variable_disputes {
		let v in 10..BenchBuilder::<T>::max_validators();

		let max_validators = BenchBuilder::<T>::max_validators();
		let max_validators_per_core = BenchBuilder::<T>::max_validators_per_core();

		let cores_with_disputed = BenchBuilder::<T>::cores() / 2;
		let cores_with_backed = BenchBuilder::<T>::cores() / 2;

		let scenario = BenchBuilder::<T>::new()
			.build(cores_with_disputed, cores_with_backed);

		let mut benchmark = scenario.data.clone();
		let dispute = benchmark.disputes.pop();

		benchmark.bitfields.clear();
		benchmark.backed_candidates.clear();
		benchmark.disputes.clear();

		benchmark.disputes.push(dispute.unwrap());
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
		assert_eq!(vote.session, scenario.session);
	}

	// The weight of one bitfield.
	enter_bitfields {
		let cores_with_disputed = BenchBuilder::<T>::cores() / 2;
		let cores_with_backed = BenchBuilder::<T>::cores() / 2;

		let scenario = BenchBuilder::<T>::new()
			.build(cores_with_disputed, cores_with_backed);

		let mut benchmark = scenario.data.clone();
		let bitfield = benchmark.bitfields.pop();

		benchmark.bitfields.clear();
		benchmark.backed_candidates.clear();
		benchmark.disputes.clear();

		benchmark.bitfields.push(bitfield.unwrap());
	}: enter(RawOrigin::None, benchmark)
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();
		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);
	}

	// Variant over `v`, the amount of validity votes for a backed candidate. This gives the weight
	// of a single backed candidate.
	enter_backed_candidates_variable {
		let v in 10..BenchBuilder::<T>::max_validators();

		let cores_with_disputed = BenchBuilder::<T>::cores() / 2;
		let cores_with_backed = BenchBuilder::<T>::cores() / 2;

		let scenario = BenchBuilder::<T>::new()
			.build(cores_with_disputed, cores_with_backed);


		let mut benchmark = scenario.data.clone();
		let backed_candidate = benchmark.backed_candidates.pop();

		benchmark.bitfields.clear();
		benchmark.backed_candidates.clear();
		benchmark.disputes.clear();

		benchmark.backed_candidates.push(backed_candidate.unwrap());
		benchmark.backed_candidates.get_mut(0).unwrap().validity_votes.drain(v as usize..);
	}: enter(RawOrigin::None, benchmark)
	verify {
		let cores = BenchBuilder::<T>::cores();
		let max_validators_per_core = BenchBuilder::<T>::max_validators_per_core();
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();
		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);
		// Ensure that there are an expected number of candidates
		assert_eq!(vote.backing_validators_per_candidate.len(), v as usize);
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), max_validators_per_core as usize /* Backing Group Size */);
		}

		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			cores as usize,
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			cores as usize,
		);
	}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
