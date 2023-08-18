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
use frame_benchmarking::benchmarks;
use pallet_message_queue as mq;

benchmarks! {
	where_clause {
		where
			T: mq::Config,
	}

	receive_upward_messages {
		let i in 1 .. 1000;

		let max_len = mq::MaxMessageLenOf::<T>::get() as usize;
		let para = 42u32.into();	// not especially important.
		let upward_messages = vec![vec![0; max_len]; i as usize];
		Pallet::<T>::receive_upward_messages(para, vec![vec![0; max_len]; 1].as_slice());
	}: { Pallet::<T>::receive_upward_messages(para, upward_messages.as_slice()) }

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext(Default::default()),
		crate::mock::Test
	);
}
