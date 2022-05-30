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
//! DATE: 2022-05-11, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// target/debug/polkadot
// benchmark
// pallet
// --chain=kusama-dev
// --steps=1
// --repeat=1
// --pallet=frame_election_provider_support
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `frame_election_provider_support`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> frame_election_provider_support::WeightInfo for WeightInfo<T> {
	fn phragmen(v: u32, t: u32, d: u32, ) -> Weight {
		(0 as Weight)
			// Standard Error: 10_477_000
			.saturating_add((91_333_000 as Weight).saturating_mul(v as Weight))
			// Standard Error: 10_477_000
			.saturating_add((10_333_000 as Weight).saturating_mul(t as Weight))
			// Standard Error: 10_477_000
			.saturating_add((515_333_000 as Weight).saturating_mul(d as Weight))
	}
	fn phragmms(_v: u32, t: u32, d: u32, ) -> Weight {
		(10_002_000_000 as Weight)
			// Standard Error: 19_078_000
			.saturating_add((4_000_000 as Weight).saturating_mul(t as Weight))
			// Standard Error: 19_078_000
			.saturating_add((594_000_000 as Weight).saturating_mul(d as Weight))
	}
	fn mms(v: u32, t: u32, d: u32, ) -> Weight {
		(0 as Weight)
			// Standard Error: 269_966_000
			.saturating_add((38_667_000 as Weight).saturating_mul(v as Weight))
			// Standard Error: 269_966_000
			.saturating_add((613_667_000 as Weight).saturating_mul(t as Weight))
			// Standard Error: 269_966_000
			.saturating_add((327_166_667_000 as Weight).saturating_mul(d as Weight))
	}
}
