//! Benchmarking setup for pallet-template
#![cfg(feature = "runtime-benchmarks")]
use super::{Pallet as AssignedSlots, *};

use frame_benchmarking::v2::*;
use frame_system::RawOrigin;
use primitives::Id as ParaId;
use frame_support::{assert_ok};

#[benchmarks]
 mod benchmarks {
     use super::*;
     use crate::{mock::TestRegistrar};
     use ::test_helpers::{dummy_head_data, dummy_validation_code};

     fn register_parachain<T: Config>(para_id: ParaId) {
        let caller: T::AccountId = whitelisted_caller();
		assert_ok!(TestRegistrar::<T>::register(
            caller,
            para_id,
            dummy_head_data(),
            dummy_validation_code(),
        ));
	}
     
    #[benchmark]
    fn assign_perm_parachain_slot() {
        let para_id = ParaId::from(2000_u32);
        let caller = RawOrigin::Root;
        register_parachain::<T>(para_id);

        let counter = PermanentSlotCount::<T>::get();
        let current_lease_period: T::BlockNumber = T::Leaser::lease_period_index(frame_system::Pallet::<T>::block_number())
            .and_then(|x| Some(x.0))
            .unwrap();
        #[extrinsic_call]
        assign_perm_parachain_slot(caller, para_id);

       
        assert_eq!(PermanentSlots::<T>::get(para_id), Some((
            current_lease_period,
            LeasePeriodOf::<T>::from(T::PermanentSlotLeasePeriodLength::get()),
        )));
        assert_eq!(PermanentSlotCount::<T>::get(), counter + 1);
    }

    impl_benchmark_test_suite!(AssignedSlots, 
        crate::assigned_slots::tests::new_test_ext(),
        crate::assigned_slots::tests::Test
    );
}
