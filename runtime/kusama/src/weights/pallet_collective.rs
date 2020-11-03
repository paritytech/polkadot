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
//! Weights for pallet_collective
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0
//! DATE: 2020-10-30, STEPS: [50, ], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 128

// Executed Command:
// ./target/release/polkadot
// benchmark
// --chain
// kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_collective
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header
// ./file_header.txt
// --output=./runtime/kusama/src/weights/


#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for pallet_collective.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Trait> pallet_collective::WeightInfo for WeightInfo<T> {
	fn set_members(m: u32, n: u32, p: u32, ) -> Weight {
		(0 as Weight)
			.saturating_add((20_744_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((71_000 as Weight).saturating_mul(n as Weight))
			.saturating_add((27_804_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().reads((1 as Weight).saturating_mul(p as Weight)))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(p as Weight)))
	}
	fn execute(b: u32, m: u32, ) -> Weight {
		(29_512_000 as Weight)
			.saturating_add((4_000 as Weight).saturating_mul(b as Weight))
			.saturating_add((112_000 as Weight).saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
	}
	fn propose_execute(b: u32, m: u32, ) -> Weight {
		(36_554_000 as Weight)
			.saturating_add((4_000 as Weight).saturating_mul(b as Weight))
			.saturating_add((220_000 as Weight).saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
	}
	fn propose_proposed(b: u32, m: u32, p: u32, ) -> Weight {
		(60_010_000 as Weight)
			.saturating_add((5_000 as Weight).saturating_mul(b as Weight))
			.saturating_add((115_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((565_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(4 as Weight))
	}
	fn vote(m: u32, ) -> Weight {
		(43_161_000 as Weight)
			.saturating_add((277_000 as Weight).saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn close_early_disapproved(m: u32, p: u32, ) -> Weight {
		(57_179_000 as Weight)
			.saturating_add((215_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((527_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn close_early_approved(b: u32, m: u32, p: u32, ) -> Weight {
		(82_532_000 as Weight)
			.saturating_add((3_000 as Weight).saturating_mul(b as Weight))
			.saturating_add((220_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((532_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn close_disapproved(m: u32, p: u32, ) -> Weight {
		(63_597_000 as Weight)
			.saturating_add((219_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((526_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn close_approved(b: u32, m: u32, p: u32, ) -> Weight {
		(88_919_000 as Weight)
			.saturating_add((3_000 as Weight).saturating_mul(b as Weight))
			.saturating_add((220_000 as Weight).saturating_mul(m as Weight))
			.saturating_add((532_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(5 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn disapprove_proposal(p: u32, ) -> Weight {
		(33_674_000 as Weight)
			.saturating_add((527_000 as Weight).saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
}
