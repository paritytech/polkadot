// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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
//! Autogenerated weights for `frame_election_provider_support`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-04-15, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// target/production/polkadot
// benchmark
// pallet
// --chain=westend-dev
// --steps=50
// --repeat=20
// --pallet=frame_election_provider_support
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `frame_election_provider_support`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> frame_election_provider_support::WeightInfo for WeightInfo<T> {
	fn phragmen(v: u32, _t: u32, d: u32, ) -> Weight {
		(0 as Weight)
			// Standard Error: 49_000
			.saturating_add((18_804_000 as Weight).saturating_mul(v as Weight))
			// Standard Error: 6_922_000
			.saturating_add((3_040_535_000 as Weight).saturating_mul(d as Weight))
	}
	fn phragmms(v: u32, _t: u32, d: u32, ) -> Weight {
		(0 as Weight)
			// Standard Error: 66_000
			.saturating_add((16_029_000 as Weight).saturating_mul(v as Weight))
			// Standard Error: 9_193_000
			.saturating_add((2_707_950_000 as Weight).saturating_mul(d as Weight))
	}
}
