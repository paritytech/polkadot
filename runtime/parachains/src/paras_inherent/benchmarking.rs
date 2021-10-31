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
use crate::{
	configuration, inclusion, initializer, paras, scheduler, session_info, shared, ParaId,
};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use sp_core::H256;
use sp_std::cmp::min;

use crate::builder::BenchBuilder;

#[cfg(not(test))]
const MAX_DISPUTED: u32 = 1_000;
#[cfg(test)]
const MAX_DISPUTED: u32 = 8;

#[cfg(not(test))]
const MAX_BACKED: u32 = 1_000;
#[cfg(test)]
const MAX_BACKED: u32 = 8;

#[cfg(not(test))]
const MAX_VALIDATORS: u32 = 1_000;
#[cfg(test)]
const MAX_VALIDATORS: u32 = 16;

// Variant over `d`, the number of cores with a disputed candidate. Remainder of cores are concluding
// and backed candidates.
benchmarks! {
	enter_backed_only {
		let b in 0..MAX_BACKED;
		let d in 0..MAX_DISPUTED;
		//let v in 5..MAX_VALIDATORS;
        let v = 200;
		let p = 5;

		let scenario = BenchBuilder::<T>::new()
			.build(v, p, b, d);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
        println!("BACKED {:?}", b);
        println!("DISPUTES {:?}", d);
        println!("Validators {:?}", v);
        println!("Per Core {:?}", p);
		let cores = v / p;
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

	// 	// Ensure that there are an expected number of candidates
	// 	assert_eq!(vote.backing_validators_per_candidate.len(), BenchBuilder::<T>::cores() as usize);

		// Ensure that there are an expected number of candidates
		assert_eq!(vote.backing_validators_per_candidate.len(), cores as usize);

		// TODO This is not the parent block, why does this work?
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), p as usize /* Backing Group Size */);
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

	// 	assert_eq!(
	// 		scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
	// 	);
	// }
}

// - no spam scenario
// - max backed candidates scenario

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
