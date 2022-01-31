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
//! Autogenerated weights for `pallet_gilt`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-01-28, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// target/release/polkadot
// benchmark
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_gilt
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/pallet_gilt.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_gilt`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_gilt::WeightInfo for WeightInfo<T> {
	// Storage: Gilt Queues (r:1 w:1)
	// Storage: Gilt QueueTotals (r:1 w:1)
	fn place_bid(l: u32, ) -> Weight {
		(37_251_000 as Weight)
			// Standard Error: 0
			.saturating_add((161_000 as Weight).saturating_mul(l as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: Gilt Queues (r:1 w:1)
	// Storage: Gilt QueueTotals (r:1 w:1)
	fn place_bid_max() -> Weight {
		(174_328_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: Gilt Queues (r:1 w:1)
	// Storage: Gilt QueueTotals (r:1 w:1)
	fn retract_bid(l: u32, ) -> Weight {
		(37_543_000 as Weight)
			// Standard Error: 0
			.saturating_add((138_000 as Weight).saturating_mul(l as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: Gilt ActiveTotal (r:1 w:1)
	fn set_target() -> Weight {
		(4_335_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: Gilt Active (r:1 w:1)
	// Storage: Gilt ActiveTotal (r:1 w:1)
	fn thaw() -> Weight {
		(54_541_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	// Storage: Gilt ActiveTotal (r:1 w:0)
	fn pursue_target_noop() -> Weight {
		(2_406_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
	}
	// Storage: Gilt ActiveTotal (r:1 w:1)
	// Storage: Gilt QueueTotals (r:1 w:1)
	// Storage: Gilt Queues (r:1 w:1)
	// Storage: Gilt Active (r:0 w:1)
	fn pursue_target_per_item(b: u32, ) -> Weight {
		(39_905_000 as Weight)
			// Standard Error: 2_000
			.saturating_add((5_920_000 as Weight).saturating_mul(b as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(b as Weight)))
	}
	// Storage: Gilt ActiveTotal (r:1 w:1)
	// Storage: Gilt QueueTotals (r:1 w:1)
	// Storage: Gilt Queues (r:1 w:1)
	// Storage: Gilt Active (r:0 w:1)
	fn pursue_target_per_queue(q: u32, ) -> Weight {
		(7_197_000 as Weight)
			// Standard Error: 8_000
			.saturating_add((11_137_000 as Weight).saturating_mul(q as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().reads((1 as Weight).saturating_mul(q as Weight)))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
			.saturating_add(T::DbWeight::get().writes((2 as Weight).saturating_mul(q as Weight)))
	}
}
