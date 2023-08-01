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

use super::{Pallet as Registrar, *};
use crate::traits::Registrar as RegistrarT;
use frame_support::assert_ok;
use frame_system::RawOrigin;
use primitives::{MAX_CODE_SIZE, MAX_HEAD_DATA_SIZE};
use runtime_parachains::{paras, shared, Origin as ParaOrigin};
use sp_runtime::traits::Bounded;

use frame_benchmarking::{account, benchmarks, whitelisted_caller};

fn assert_last_event<T: Config>(generic_event: <T as Config>::RuntimeEvent) {
	let events = frame_system::Pallet::<T>::events();
	let system_event: <T as frame_system::Config>::RuntimeEvent = generic_event.into();
	// compare to the last event record
	let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
	assert_eq!(event, &system_event);
}

fn register_para<T: Config>(id: u32) -> ParaId {
	let para = ParaId::from(id);
	let genesis_head = Registrar::<T>::worst_head_data();
	let validation_code = Registrar::<T>::worst_validation_code();
	let caller: T::AccountId = whitelisted_caller();
	T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
	assert_ok!(Registrar::<T>::reserve(RawOrigin::Signed(caller.clone()).into()));
	assert_ok!(Registrar::<T>::register(
		RawOrigin::Signed(caller).into(),
		para,
		genesis_head,
		validation_code.clone()
	));
	assert_ok!(runtime_parachains::paras::Pallet::<T>::add_trusted_validation_code(
		frame_system::Origin::<T>::Root.into(),
		validation_code,
	));
	return para
}

fn para_origin(id: u32) -> ParaOrigin {
	ParaOrigin::Parachain(id.into())
}

// This function moves forward to the next scheduled session for parachain lifecycle upgrades.
fn next_scheduled_session<T: Config>() {
	shared::Pallet::<T>::set_session_index(shared::Pallet::<T>::scheduled_session());
	paras::Pallet::<T>::test_on_new_session();
}

benchmarks! {
	where_clause { where ParaOrigin: Into<<T as frame_system::Config>::RuntimeOrigin> }

	reserve {
		let caller: T::AccountId = whitelisted_caller();
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
	}: _(RawOrigin::Signed(caller.clone()))
	verify {
		assert_last_event::<T>(Event::<T>::Reserved { para_id: LOWEST_PUBLIC_ID, who: caller }.into());
		assert!(Paras::<T>::get(LOWEST_PUBLIC_ID).is_some());
		assert_eq!(paras::Pallet::<T>::lifecycle(LOWEST_PUBLIC_ID), None);
	}

	register {
		let para = LOWEST_PUBLIC_ID;
		let genesis_head = Registrar::<T>::worst_head_data();
		let validation_code = Registrar::<T>::worst_validation_code();
		let caller: T::AccountId = whitelisted_caller();
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		assert_ok!(Registrar::<T>::reserve(RawOrigin::Signed(caller.clone()).into()));
	}: _(RawOrigin::Signed(caller.clone()), para, genesis_head, validation_code.clone())
	verify {
		assert_last_event::<T>(Event::<T>::Registered{ para_id: para, manager: caller }.into());
		assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Onboarding));
		assert_ok!(runtime_parachains::paras::Pallet::<T>::add_trusted_validation_code(
			frame_system::Origin::<T>::Root.into(),
			validation_code,
		));
		next_scheduled_session::<T>();
		assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Parathread));
	}

	force_register {
		let manager: T::AccountId = account("manager", 0, 0);
		let deposit = 0u32.into();
		let para = ParaId::from(69);
		let genesis_head = Registrar::<T>::worst_head_data();
		let validation_code = Registrar::<T>::worst_validation_code();
	}: _(RawOrigin::Root, manager.clone(), deposit, para, genesis_head, validation_code.clone())
	verify {
		assert_last_event::<T>(Event::<T>::Registered { para_id: para, manager }.into());
		assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Onboarding));
		assert_ok!(runtime_parachains::paras::Pallet::<T>::add_trusted_validation_code(
			frame_system::Origin::<T>::Root.into(),
			validation_code,
		));
		next_scheduled_session::<T>();
		assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Parathread));
	}

	deregister {
		let para = register_para::<T>(LOWEST_PUBLIC_ID.into());
		next_scheduled_session::<T>();
		let caller: T::AccountId = whitelisted_caller();
	}: _(RawOrigin::Signed(caller), para)
	verify {
		assert_last_event::<T>(Event::<T>::Deregistered { para_id: para }.into());
	}

	swap {
		let parathread = register_para::<T>(LOWEST_PUBLIC_ID.into());
		let parachain = register_para::<T>((LOWEST_PUBLIC_ID + 1).into());

		let parachain_origin = para_origin(parachain.into());

		// Actually finish registration process
		next_scheduled_session::<T>();

		// Upgrade the parachain
		Registrar::<T>::make_parachain(parachain)?;
		next_scheduled_session::<T>();

		assert_eq!(paras::Pallet::<T>::lifecycle(parachain), Some(ParaLifecycle::Parachain));
		assert_eq!(paras::Pallet::<T>::lifecycle(parathread), Some(ParaLifecycle::Parathread));

		let caller: T::AccountId = whitelisted_caller();
		Registrar::<T>::swap(parachain_origin.into(), parachain, parathread)?;
	}: _(RawOrigin::Signed(caller.clone()), parathread, parachain)
	verify {
		next_scheduled_session::<T>();
		// Swapped!
		assert_eq!(paras::Pallet::<T>::lifecycle(parachain), Some(ParaLifecycle::Parathread));
		assert_eq!(paras::Pallet::<T>::lifecycle(parathread), Some(ParaLifecycle::Parachain));
	}

	schedule_code_upgrade {
		let b in 1 .. MAX_CODE_SIZE;
		let new_code = ValidationCode(vec![0; b as usize]);
		let para_id = ParaId::from(1000);
	}: _(RawOrigin::Root, para_id, new_code)

	set_current_head {
		let b in 1 .. MAX_HEAD_DATA_SIZE;
		let new_head = HeadData(vec![0; b as usize]);
		let para_id = ParaId::from(1000);
	}: _(RawOrigin::Root, para_id, new_head)

	impl_benchmark_test_suite!(
		Registrar,
		crate::integration_tests::new_test_ext(),
		crate::integration_tests::Test,
	);
}
