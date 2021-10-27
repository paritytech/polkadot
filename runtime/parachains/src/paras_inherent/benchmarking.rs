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
use crate::{configuration, inclusion, initializer, paras, scheduler, session_info, shared};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use sp_core::H256;
use sp_runtime::{
	generic::Digest,
	traits::{One, Zero},
	RuntimeAppPublic,
};
use sp_std::{collections::btree_map::BTreeMap as HashMap, convert::TryInto};
use sp_runtime::RuntimeAppPublic;

use crate::builder::BenchBuilder;

// Variant over `d`, the number of cores with a disputed candidate. Remainder of cores are concluding
// and backed candidates.
benchmarks! {
	enter_dispute_dominant {
		let d in 0..BenchBuilder::<T>::cores();

		let backed_and_concluding = BenchBuilder::<T>::cores() - d;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(backed_and_concluding, d);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);

		// TODO This is not the parent block, why does this work?
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), 5 /* Backing Group Size */);
		}

		// exactly the disputed cores are scheduled since they where freed.
		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	enter_disputes_only {
		//let d in 0..BenchBuilder::<T>::cores();
		let d = BenchBuilder::<T>::cores();
		let b = 0;

		log::info!(target: LOG_TARGET, "a");
		let backed_and_concluding = 0;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
		.build(backed_and_concluding, d);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);

		// Ensure that there are an expected number of candidates
		assert_eq!(vote.backing_validators_per_candidate.len(), BenchBuilder::<T>::cores() as usize);

		// TODO This is not the parent block, why does this work?
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), 5 /* Backing Group Size */);
		}

		// exactly the disputed cores are scheduled since they where freed.

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	// Variant of over `b`, the number of cores concluding and immediately receiving a new
	// backed candidate. Remainder of cores are occupied by disputes.
	enter_backed_dominant {
		let b in 0..BenchBuilder::<T>::cores();

		let disputed = BenchBuilder::<T>::cores() - b;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);

		// Ensure that there are an expected number of candidates
		assert_eq!(vote.backing_validators_per_candidate.len(), BenchBuilder::<T>::cores() as usize);

		// TODO This is not the parent block, why does this work?
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), 5 /* Backing Group Size */);
		}

		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			b as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			b as usize
		);

		// exactly the disputed cores are scheduled since they where freed.

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	enter_backed_only {
		//let b in 0..BenchBuilder::<T>::cores();
		let b = BenchBuilder::<T>::cores();

		let disputed = 0;

		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// Assert that the block was not discarded
		assert!(Included::<T>::get().is_some());
		// Assert that there are on-chain votes that got scraped
		let onchain_votes = OnChainVotes::<T>::get();
		assert!(onchain_votes.is_some());
		let vote = onchain_votes.unwrap();

		// Ensure that the votes are for the correct session
		assert_eq!(vote.session, scenario.session);

		// Ensure that there are an expected number of candidates
		assert_eq!(vote.backing_validators_per_candidate.len(), BenchBuilder::<T>::cores() as usize);

		// TODO This is not the parent block, why does this work?
		let header = BenchBuilder::<T>::header(scenario.block_number.clone());
		// Traverse candidates and assert descriptors are as expected
		for (para_id, backing_validators) in vote.backing_validators_per_candidate.iter().enumerate() {
			let descriptor = backing_validators.0.descriptor();
			assert_eq!(ParaId::from(para_id), descriptor.para_id);
			assert_eq!(header.hash(), descriptor.relay_parent);
			assert_eq!(backing_validators.1.len(), 5 /* Backing Group Size */);
		}

		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			b as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			b as usize
		);

		// exactly the disputed cores are scheduled since they where freed.

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}
}

// - no spam scenario
// - max backed candidates scenario

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
