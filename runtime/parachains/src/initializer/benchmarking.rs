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
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_system::{DigestItemOf, RawOrigin};
use primitives::v1::ConsensusLog;

// Random large number for the digest
const DIGEST_MAX_LEN: u32 = 65536;

benchmarks! {
	force_approve {
		let d in 0 .. DIGEST_MAX_LEN;
		for _ in 0 .. d {
			<frame_system::Pallet<T>>::deposit_log(ConsensusLog::ForceApprove(d).into());
		}
	}: _(RawOrigin::Root, d + 1)
	verify {
		assert_eq!(
			<frame_system::Pallet<T>>::digest().logs.last().unwrap(),
			&<DigestItemOf<T>>::from(ConsensusLog::ForceApprove(d + 1)),
		);
	}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
