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
//! Autogenerated weights for `runtime_common::slots`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-10-25, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=westend-dev
// --steps=50
// --repeat=20
// --pallet=runtime_common::slots
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/runtime_common_slots.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_common::slots`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_common::slots::WeightInfo for WeightInfo<T> {
	// Storage: Slots Leases (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	fn force_lease() -> Weight {
		// Minimum execution time: 30_044 nanoseconds.
		Weight::from_ref_time(30_999_000 as u64)
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
	// Storage: Paras Parachains (r:1 w:0)
	// Storage: Slots Leases (r:101 w:100)
	// Storage: Paras ParaLifecycles (r:101 w:101)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras ActionsQueue (r:1 w:1)
	// Storage: Registrar Paras (r:100 w:100)
	/// The range of component `c` is `[1, 100]`.
	/// The range of component `t` is `[1, 100]`.
	fn manage_lease_period_start(c: u32, t: u32, ) -> Weight {
		// Minimum execution time: 541_004 nanoseconds.
		Weight::from_ref_time(544_395_000 as u64)
			// Standard Error: 67_401
			.saturating_add(Weight::from_ref_time(2_009_663 as u64).saturating_mul(c as u64))
			// Standard Error: 67_401
			.saturating_add(Weight::from_ref_time(11_680_248 as u64).saturating_mul(t as u64))
			.saturating_add(T::DbWeight::get().reads(4 as u64))
			.saturating_add(T::DbWeight::get().reads((1 as u64).saturating_mul(c as u64)))
			.saturating_add(T::DbWeight::get().reads((3 as u64).saturating_mul(t as u64)))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
			.saturating_add(T::DbWeight::get().writes((1 as u64).saturating_mul(c as u64)))
			.saturating_add(T::DbWeight::get().writes((3 as u64).saturating_mul(t as u64)))
	}
	// Storage: Slots Leases (r:1 w:1)
	// Storage: System Account (r:8 w:8)
	fn clear_all_leases() -> Weight {
		// Minimum execution time: 90_911 nanoseconds.
		Weight::from_ref_time(92_388_000 as u64)
			.saturating_add(T::DbWeight::get().reads(9 as u64))
			.saturating_add(T::DbWeight::get().writes(9 as u64))
	}
	// Storage: Slots Leases (r:1 w:0)
	// Storage: Paras ParaLifecycles (r:1 w:1)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras ActionsQueue (r:1 w:1)
	// Storage: Registrar Paras (r:1 w:1)
	fn trigger_onboard() -> Weight {
		// Minimum execution time: 28_595 nanoseconds.
		Weight::from_ref_time(29_454_000 as u64)
			.saturating_add(T::DbWeight::get().reads(5 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}
}
