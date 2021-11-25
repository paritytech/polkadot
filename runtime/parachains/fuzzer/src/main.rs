// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! # Running
//! Running this fuzzer can be done with `cargo hfuzz run paras-inherent. `honggfuzz` CLI options can
//! be used by setting `HFUZZ_RUN_ARGS`, such as `-n 4` to use 4 threads.
//!
//! # Debugging a panic
//! Once a panic is found, it can be debugged with
//! `cargo hfuzz run-debug fixed_point hfuzz_workspace/paras-inherent/*.fuzz`.
//!
//! # More information
//! More information about `honggfuzz` can be found
//! [here](https://docs.rs/honggfuzz/).

use honggfuzz::fuzz;
use polkadot_runtime_parachains::{
	builder::BenchBuilder,
	mock::{new_test_ext, MockGenesisConfig, Test, ParaInherent}
};
use sp_std::collections::btree_map::BTreeMap;
use frame_support::assert_ok;

const CORES: u32 = 15;
const MAX_VALIDATORS_PER_CORE: u32 = 3;
const VALIDATOR_COUNT: u32 = CORES * MAX_VALIDATORS_PER_CORE;
const SESSION_VARIABLILITY: u32 = 5;

fn main() {
	loop {
		fuzz!(|data: (u32, u32, u32, u32)| {
			let (seed, disputes_seed, backed_seed, votes_seed) = data;
			// variant over
			// * number of disputes
			//  * dispute sessions
			//  * local vs remote disputes
			// * number of backed candidates and availability votes
			// * signature quality
			// * other stuff

			let disputes_count = disputes_seed % CORES;
			let backed_and_concluding_count = CORES - disputes_count;

			let validator_count
				= CORES * MAX_VALIDATORS_PER_CORE;
			let builder = BenchBuilder::<Test>::new()
				.set_max_validators_per_core(MAX_VALIDATORS_PER_CORE)
				.set_max_validators(validator_count);
				// .set_dispute_statements

			let backed_and_concluding: BTreeMap<_, _> = (0..backed_and_concluding_count).map(|i| {
				let vote_count = (votes_seed % (MAX_VALIDATORS_PER_CORE + 1))
					// make sure we don't error due to not having enough votes
					.max(BenchBuilder::<Test>::fallback_min_validity_votes());

				(i, vote_count)
			})
			.collect();

			let dispute_sessions: Vec<_> = (0..disputes_count).map(|i|
				if disputes_seed % 2 == 0 {
					(i + disputes_seed) % SESSION_VARIABLILITY
				} else {
					2
				}
			)
			.collect();

			let code_upgrade = if seed % 2 == 0 {
						Some(seed)
					} else {
						None
					};

			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let data = builder.build(
					backed_and_concluding, dispute_sessions.as_slice(), code_upgrade
				).data;

				assert_ok!(
					ParaInherent::enter(
						frame_system::RawOrigin::None.into(),
						data
					)
				);
			});

		});
	}
}
