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
//! Autogenerated weights for claims
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0
//! DATE: 2020-12-08, STEPS: [50, ], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 128

// Executed Command:
// target/release/polkadot
// benchmark
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=claims
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/


#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for claims.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> claims::WeightInfo for WeightInfo<T> {
	fn claim(_u: u32, ) -> Weight {
		(294_878_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(7 as Weight))
			.saturating_add(T::DbWeight::get().writes(7 as Weight))
	}
	fn mint_claim(_c: u32, ) -> Weight {
		(17_554_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(4 as Weight))
	}
	fn claim_attest(_u: u32, ) -> Weight {
		(298_521_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(7 as Weight))
			.saturating_add(T::DbWeight::get().writes(7 as Weight))
	}
	fn attest(_u: u32, ) -> Weight {
		(143_210_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(8 as Weight))
			.saturating_add(T::DbWeight::get().writes(8 as Weight))
	}
	fn validate_unsigned_claim(_c: u32, ) -> Weight {
		(175_333_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
	}
	fn validate_unsigned_claim_attest(_c: u32, ) -> Weight {
		(177_906_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
	}
	fn validate_prevalidate_attests(_c: u32, ) -> Weight {
		(13_189_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
	}
	fn keccak256(i: u32, ) -> Weight {
		(2_604_000 as Weight)
			// Standard Error: 0
			.saturating_add((815_000 as Weight).saturating_mul(i as Weight))
	}
	fn eth_recover(i: u32, ) -> Weight {
		(24_251_000 as Weight)
			// Standard Error: 80_000
			.saturating_add((157_040_000 as Weight).saturating_mul(i as Weight))
	}
}
