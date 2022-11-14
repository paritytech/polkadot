// Copyright 2020 Parity Technologies (UK) Ltd.
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
use frame_benchmarking::{benchmarks, BenchmarkError, BenchmarkResult};
use frame_system::RawOrigin;
use sp_runtime::traits::One;

benchmarks! {
	receive_upward_messages {
		receive_upward_messages(config, para, vec![vec![0; MAX_UPWARD_MESSAGE_SIZE_BOUND]; 1])

		let config = configuration::ActiveConfig::<T>::get();
		let para = 42;	// not especially important.

		let i in 1..1000;

		let upward_messages = vec![vec![0; MAX_UPWARD_MESSAGE_SIZE_BOUND]; i];
	}: receive_upward_messages(config, para, upward_messages)

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext(Default::default()),
		crate::mock::Test
	);
}
