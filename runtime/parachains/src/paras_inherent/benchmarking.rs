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

use crate::builder::BenchBuilder;

// Variant over `d`, the number of cores with a disputed candidate. Remainder of cores are concluding
// and backed candidates.
benchmarks! {
	/*enter_dispute_dominant {
		let d in 0..BenchBuilder::<T>::cores();

		let backed_and_concluding = BenchBuilder::<T>::cores() - d;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(backed_and_concluding, d);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// check that the disputes storage has updated as expected.

		// TODO
		// if d > 1 {
		// 	let spam_slots = disputes::Pallet::<T>::spam_slots(&scenario.current_session).unwrap();
		// 	assert!(
		// 		// we expect the first 1/3rd of validators to have maxed out spam slots. Sub 1 for when
		// 		// there is an odd number of validators.
		// 		// TODO
		// 		&spam_slots[..(BenchBuilder::<T>::statement_spam_thresh() - 1) as usize]
		// 		.iter()
		// 		.all(|n| *n == config.dispute_max_spam_slots)
		// 	);
		// 	assert!(
		// 		&spam_slots[BenchBuilder::<T>::statement_spam_thresh() as usize ..]
		// 		.iter()
		// 		.all(|n| *n == 0)
		// 	);
		// }

		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			backed_and_concluding as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			backed_and_concluding as usize
		);

		// max possible number of cores have been scheduled.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), d as usize);

		// all cores are occupied by a parachain.
		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}*/

	// enter_disputes_only {
	// 	//let d in 0..BenchBuilder::<T>::cores();
	// 	let d = BenchBuilder::<T>::cores();

	// 	log::info!(target: LOG_TARGET, "a");
	// 	let backed_and_concluding = 0;

	// 	let config = configuration::Pallet::<T>::config();
	// 	let scenario = BenchBuilder::<T>::new()
	// 		.build(backed_and_concluding, d);
	// 	let b = frame_system::Pallet::<T>::block_number();
	// 	println!("WE ARE BENCHING NOW {:?}", b);
	// }: enter(RawOrigin::None, scenario.data.clone())
	// verify {
	// 	log::warn!(target: LOG_TARGET, "y");
	// 	// check that the disputes storage has updated as expected.

	// 	// TODO
	// 	// if d > 1 {
	// 	// 	let spam_slots = disputes::Pallet::<T>::spam_slots(&scenario.current_session).unwrap();
	// 	// 	assert!(
	// 	// 		// we expect the first 1/3rd of validators to have maxed out spam slots. Sub 1 for when
	// 	// 		// there is an odd number of validators.
	// 	// 		// TODO
	// 	// 		&spam_slots[..(BenchBuilder::<T>::statement_spam_thresh() - 1) as usize]
	// 	// 		.iter()
	// 	// 		.all(|n| *n == config.dispute_max_spam_slots)
	// 	// 	);
	// 	// 	assert!(
	// 	// 		&spam_slots[BenchBuilder::<T>::statement_spam_thresh() as usize ..]
	// 	// 		.iter()
	// 	// 		.all(|n| *n == 0)
	// 	// 	);
	// 	// }


	// 	// pending availability data is removed when disputes are collected.
	// 	assert_eq!(
	// 		inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
	// 		backed_and_concluding as usize
	// 	);
	// 	assert_eq!(
	// 		inclusion::PendingAvailability::<T>::iter().count(),
	// 		backed_and_concluding as usize
	// 	);

	// 	// max possible number of cores have been scheduled.
	// 	assert_eq!(scheduler::Scheduled::<T>::get().len(), d as usize);

	// 	// all cores are occupied by a parachain.
	// 	assert_eq!(
	// 		scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
	// 	);
	// }

	// Variant of over `b`, the number of cores concluding and immediately receiving a new
	// backed candidate. Remainder of cores are occupied by disputes.
	/*enter_backed_dominant {
		let b in 0..BenchBuilder::<T>::cores();

		let disputed = BenchBuilder::<T>::cores() - b;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			b as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			b as usize
		);

		// exactly the disputed cores are schedule since they where freed.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), disputed as usize);

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}
*/
	enter_backed_only {
		let b in 0..BenchBuilder::<T>::cores();

		let disputed = 0;

		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
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
