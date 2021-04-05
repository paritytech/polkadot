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

//! Benchmarking for the xcm-benchmarks wrapper pallet.

use super::*;

use frame_system::RawOrigin;
use frame_benchmarking::{benchmarks, whitelisted_caller, impl_benchmark_test_suite};
#[allow(unused)]
use crate::Pallet as XcmBenchmarks;
use xcm_executor::XcmExecutor;
use xcm::v0::MultiLocation;


benchmarks! {
	execute_xcm {
		let origin = MultiLocation::default();
		let message = Xcm::default();
		let weight_limit = Weight::max_value();
	}: {
		XcmExecutor::<T::XcmConfig>::execute_xcm(origin, message, weight_limit)?;
	}
}

impl_benchmark_test_suite!(
	XcmBenchmarks,
	crate::mock::new_test_ext(),
	crate::mock::Test,
);
