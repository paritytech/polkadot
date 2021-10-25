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
use crate::{configuration::HostConfiguration, shared};
use frame_benchmarking::benchmarks;
use frame_system::RawOrigin;
use primitives::v1::{HeadData, Id as ParaId, ValidationCode, MAX_CODE_SIZE, MAX_HEAD_DATA_SIZE};
use sp_runtime::traits::{One, Saturating};

// 2 ^ 10, because binary search time complexity is O(log(2, n)) and n = 1024 gives us a big and
// round number.
// Due to the limited number of parachains, the number of pruning, upcoming upgrades and cooldowns
// shouldn't exceed this number.
const SAMPLE_SIZE: u32 = 1024;

fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
	let events = frame_system::Pallet::<T>::events();
	let system_event: <T as frame_system::Config>::Event = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
	assert_eq!(event, &system_event);
}

fn generate_disordered_pruning<T: Config>() {
	let mut needs_pruning = Vec::new();

	for i in 0..SAMPLE_SIZE {
		let id = ParaId::from(i);
		let block_number = T::BlockNumber::from(1000u32);
		needs_pruning.push((id, block_number));
	}

	<Pallet<T> as Store>::PastCodePruning::put(needs_pruning);
}

fn generate_disordered_upgrades<T: Config>() {
	let mut upgrades = Vec::new();
	let mut cooldowns = Vec::new();

	for i in 0..SAMPLE_SIZE {
		let id = ParaId::from(i);
		let block_number = T::BlockNumber::from(1000u32);
		upgrades.push((id, block_number));
		cooldowns.push((id, block_number));
	}

	<Pallet<T> as Store>::UpcomingUpgrades::put(upgrades);
	<Pallet<T> as Store>::UpgradeCooldowns::put(cooldowns);
}

fn generate_disordered_actions_queue<T: Config>() {
	let mut queue = Vec::new();
	let next_session = shared::Pallet::<T>::session_index().saturating_add(One::one());

	for _ in 0..SAMPLE_SIZE {
		let id = ParaId::from(1000);
		queue.push(id);
	}

	<Pallet<T> as Store>::ActionsQueue::mutate(next_session, |v| {
		*v = queue;
	});
}

benchmarks! {
	force_set_current_code {
		let c in 1 .. MAX_CODE_SIZE;
		let new_code = ValidationCode(vec![0; c as usize]);
		let para_id = ParaId::from(c as u32);
		generate_disordered_pruning::<T>();
	}: _(RawOrigin::Root, para_id, new_code)
	verify {
		assert_last_event::<T>(Event::CurrentCodeUpdated(para_id).into());
	}
	force_set_current_head {
		let s in 1 .. MAX_HEAD_DATA_SIZE;
		let new_head = HeadData(vec![0; s as usize]);
		let para_id = ParaId::from(1000);
	}: _(RawOrigin::Root, para_id, new_head)
	verify {
		assert_last_event::<T>(Event::CurrentHeadUpdated(para_id).into());
	}
	force_schedule_code_upgrade {
		let c in 1 .. MAX_CODE_SIZE;
		let new_code = ValidationCode(vec![0; c as usize]);
		let para_id = ParaId::from(c as u32);
		let block = T::BlockNumber::from(c);
		generate_disordered_upgrades::<T>();
	}: _(RawOrigin::Root, para_id, new_code, block)
	verify {
		assert_last_event::<T>(Event::CodeUpgradeScheduled(para_id).into());
	}
	force_note_new_head {
		let s in 1 .. MAX_HEAD_DATA_SIZE;
		let para_id = ParaId::from(1000);
		let new_head = HeadData(vec![0; s as usize]);
		// schedule an expired code upgrade for this para_id so that force_note_new_head would use
		// the worst possible code path
		let expired = frame_system::Pallet::<T>::block_number().saturating_sub(One::one());
		let config = HostConfiguration::<T::BlockNumber>::default();
		generate_disordered_pruning::<T>();
		Pallet::<T>::schedule_code_upgrade(para_id, ValidationCode(vec![0]), expired, &config);
	}: _(RawOrigin::Root, para_id, new_head)
	verify {
		assert_last_event::<T>(Event::NewHeadNoted(para_id).into());
	}
	force_queue_action {
		let para_id = ParaId::from(1000);
		generate_disordered_actions_queue::<T>();
	}: _(RawOrigin::Root, para_id)
	verify {
		let next_session = crate::shared::Pallet::<T>::session_index().saturating_add(One::one());
		assert_last_event::<T>(Event::ActionQueued(para_id, next_session).into());
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext(Default::default()),
		crate::mock::Test
	);
}
