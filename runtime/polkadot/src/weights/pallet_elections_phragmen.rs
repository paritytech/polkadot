// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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
//! Weights for pallet_elections_phragmen
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0
//! DATE: 2020-10-29, STEPS: [50, ], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 128

// Executed Command:
// ./target/release/polkadot
// benchmark
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_elections_phragmen
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header
// ./file_header.txt
// --output=./runtime/polkadot/src/weights/


#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for pallet_elections_phragmen.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Trait> pallet_elections_phragmen::WeightInfo for WeightInfo<T> {
	fn vote_equal(v: u32, ) -> Weight {
		0
	}
	fn vote_more(v: u32, ) -> Weight {
		0
	}
	fn vote_less(v: u32, ) -> Weight {
		0
	}
	fn remove_voter() -> Weight {
		0
	}
	fn submit_candidacy(c: u32, ) -> Weight {
		0
	}
	fn renounce_candidacy_candidate(c: u32, ) -> Weight {
		0
	}
	fn renounce_candidacy_members() -> Weight {
		0
	}
	fn renounce_candidacy_runners_up() -> Weight {
		0
	}
	fn remove_member_with_replacement() -> Weight {
		0
	}
	fn remove_member_wrong_refund() -> Weight {
		0
	}
	fn clean_defunct_voters(v: u32, _d: u32, ) -> Weight {
		0
	}
	fn election_phragmen(c: u32, v: u32, e: u32, ) -> Weight {
		0
	}
}
