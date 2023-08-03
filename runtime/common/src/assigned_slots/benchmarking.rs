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

//! Benchmarking for assigned_slots pallet

#![cfg(feature = "runtime-benchmarks")]
use super::{Pallet as AssignedSlots, *};

use frame_benchmarking::v2::*;
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use primitives::Id as ParaId;

#[benchmarks]
mod benchmarks {
	use super::*;

	#[benchmark]
	fn assign_perm_parachain_slot() {
		let para_id = ParaId::from(1_u32);
		let caller = RawOrigin::Root;
		let who: T::AccountId = whitelisted_caller();
		let worst_validation_code = T::Registrar::worst_validation_code();
		let worst_head_data = T::Registrar::worst_head_data();
		let _ = T::Registrar::register(who, para_id, worst_head_data, worst_validation_code);

		let counter = PermanentSlotCount::<T>::get();
		let current_lease_period: BlockNumberFor<T> =
			T::Leaser::lease_period_index(frame_system::Pallet::<T>::block_number())
				.and_then(|x| Some(x.0))
				.unwrap();
		#[extrinsic_call]
		assign_perm_parachain_slot(caller, para_id);

		assert_eq!(
			PermanentSlots::<T>::get(para_id),
			Some((
				current_lease_period,
				LeasePeriodOf::<T>::from(T::PermanentSlotLeasePeriodLength::get()),
			))
		);
		assert_eq!(PermanentSlotCount::<T>::get(), counter + 1);
	}

	#[benchmark]
	fn assign_temp_parachain_slot() {
		let para_id = ParaId::from(2_u32);
		let caller = RawOrigin::Root;
		let who: T::AccountId = whitelisted_caller();
		let worst_validation_code = T::Registrar::worst_validation_code();
		let worst_head_data = T::Registrar::worst_head_data();
		let _ = T::Registrar::register(who, para_id, worst_head_data, worst_validation_code);

		let current_lease_period: BlockNumberFor<T> =
			T::Leaser::lease_period_index(frame_system::Pallet::<T>::block_number())
				.and_then(|x| Some(x.0))
				.unwrap();

		let counter = TemporarySlotCount::<T>::get();
		#[extrinsic_call]
		assign_temp_parachain_slot(caller, para_id, SlotLeasePeriodStart::Current);

		let tmp = ParachainTemporarySlot {
			manager: whitelisted_caller(),
			period_begin: current_lease_period,
			period_count: LeasePeriodOf::<T>::from(T::TemporarySlotLeasePeriodLength::get()),
			last_lease: Some(BlockNumberFor::<T>::zero()),
			lease_count: 1,
		};
		assert_eq!(TemporarySlots::<T>::get(para_id), Some(tmp));
		assert_eq!(TemporarySlotCount::<T>::get(), counter + 1);
	}

	#[benchmark]
	fn unassign_parachain_slot() {
		let para_id = ParaId::from(3_u32);
		let caller = RawOrigin::Root;
		let who: T::AccountId = whitelisted_caller();
		let worst_validation_code = T::Registrar::worst_validation_code();
		let worst_head_data = T::Registrar::worst_head_data();
		let _ = T::Registrar::register(who, para_id, worst_head_data, worst_validation_code);
		let _ = AssignedSlots::<T>::assign_temp_parachain_slot(
			caller.clone().into(),
			para_id,
			SlotLeasePeriodStart::Current,
		);

		let counter = TemporarySlotCount::<T>::get();
		#[extrinsic_call]
		unassign_parachain_slot(caller, para_id);

		assert_eq!(TemporarySlots::<T>::get(para_id), None);
		assert_eq!(TemporarySlotCount::<T>::get(), counter - 1);
	}

	#[benchmark]
	fn set_max_permanent_slots() {
		let caller = RawOrigin::Root;
		#[extrinsic_call]
		set_max_permanent_slots(caller, u32::MAX);

		assert_eq!(MaxPermanentSlots::<T>::get(), u32::MAX);
	}

	#[benchmark]
	fn set_max_temporary_slots() {
		let caller = RawOrigin::Root;
		#[extrinsic_call]
		set_max_temporary_slots(caller, u32::MAX);

		assert_eq!(MaxTemporarySlots::<T>::get(), u32::MAX);
	}

	impl_benchmark_test_suite!(
		AssignedSlots,
		crate::assigned_slots::tests::new_test_ext(),
		crate::assigned_slots::tests::Test
	);
}
