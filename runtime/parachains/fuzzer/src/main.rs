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
use polkadot-runtime-parachains;
use std::convert::From;

const CORES: u32 = 10;

/// Actions of a `SortedListProvider` that we fuzz.
enum Action {
	Insert,
	Update,
	Remove,
}

impl From<u32> for Action {
	fn from(v: u32) -> Self {
		let num_variants = Self::Remove as u32 + 1;
		match v % num_variants {
			_x if _x == Action::Insert as u32 => Action::Insert,
			_x if _x == Action::Update as u32 => Action::Update,
			_x if _x == Action::Remove as u32 => Action::Remove,
			_ => unreachable!(),
		}
	}
}

fn main() {
	ExtBuilder::default().build_and_execute(|| loop {
		fuzz!(|data: (u32, u32, u32)| {
			let (disputes_seed, backed_seed, signature_seed) = data;
			// variant over
			// * number of disputes
			//  * dispute sessions
			//  * local vs remote disputes
			// * number of backed candidates and availability votes
			// * signature quality
			// * other stuff

			BenchBuilder::<Test>::new()

		})
	});
}
