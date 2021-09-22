use super::*;

use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};

benchmarks! {
	force_unfreeze {
		Frozen::<T>::set(None)
	}: _(RawOrigin::Root)
	verify {
		assert!(Frozen::<T>::get().is_none())
	}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
