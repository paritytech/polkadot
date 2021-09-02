// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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
//! Autogenerated weights for `pallet_tips`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-08-18, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 128

// Executed Command:
// target/release/polkadot
// benchmark
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_tips
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/pallet_tips.rs


#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_tips`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_tips::WeightInfo for WeightInfo<T> {
	// Storage: Treasury Reasons (r:1 w:1)
	// Storage: Treasury Tips (r:1 w:1)
	fn report_awesome(r: u32, ) -> Weight {
		(48_769_000 as Weight)
			// Standard Error: 0
			.saturating_add((2_000 as Weight).saturating_mul(r as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: Treasury Tips (r:1 w:1)
	// Storage: Treasury Reasons (r:0 w:1)
	fn retract_tip() -> Weight {
		(44_512_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: PhragmenElection Members (r:1 w:0)
	// Storage: Treasury Reasons (r:1 w:1)
	// Storage: Treasury Tips (r:0 w:1)
	fn tip_new(r: u32, t: u32, ) -> Weight {
		(30_407_000 as Weight)
			// Standard Error: 0
			.saturating_add((2_000 as Weight).saturating_mul(r as Weight))
			// Standard Error: 0
			.saturating_add((109_000 as Weight).saturating_mul(t as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: PhragmenElection Members (r:1 w:0)
	// Storage: Treasury Tips (r:1 w:1)
	fn tip(t: u32, ) -> Weight {
		(18_686_000 as Weight)
			// Standard Error: 0
			.saturating_add((524_000 as Weight).saturating_mul(t as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: Treasury Tips (r:1 w:1)
	// Storage: PhragmenElection Members (r:1 w:0)
	// Storage: System Account (r:1 w:1)
	// Storage: Treasury Reasons (r:0 w:1)
	fn close_tip(t: u32, ) -> Weight {
		(80_571_000 as Weight)
			// Standard Error: 1_000
			.saturating_add((299_000 as Weight).saturating_mul(t as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	// Storage: Treasury Tips (r:1 w:1)
	// Storage: Treasury Reasons (r:0 w:1)
	fn slash_tip(t: u32, ) -> Weight {
		(24_214_000 as Weight)
			// Standard Error: 0
			.saturating_add((2_000 as Weight).saturating_mul(t as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
}
