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
//! Autogenerated weights for `pallet_scheduler`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-22, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm4`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_scheduler
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_scheduler`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_scheduler::WeightInfo for WeightInfo<T> {
	/// Storage: Scheduler IncompleteSince (r:1 w:1)
	/// Proof: Scheduler IncompleteSince (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn service_agendas_base() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `69`
		//  Estimated: `499`
		// Minimum execution time: 4_806 nanoseconds.
		Weight::from_parts(5_035_000, 0)
			.saturating_add(Weight::from_parts(0, 499))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 50]`.
	fn service_agenda_base(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `148 + s * (177 ±0)`
		//  Estimated: `41438`
		// Minimum execution time: 4_118 nanoseconds.
		Weight::from_parts(8_762_405, 0)
			.saturating_add(Weight::from_parts(0, 41438))
			// Standard Error: 5_025
			.saturating_add(Weight::from_parts(693_418, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	fn service_task_base() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 5_240 nanoseconds.
		Weight::from_parts(5_505_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: Preimage PreimageFor (r:1 w:1)
	/// Proof: Preimage PreimageFor (max_values: None, max_size: Some(4194344), added: 4196819, mode: Measured)
	/// Storage: Preimage StatusFor (r:1 w:1)
	/// Proof: Preimage StatusFor (max_values: None, max_size: Some(91), added: 2566, mode: MaxEncodedLen)
	/// The range of component `s` is `[128, 4194304]`.
	fn service_task_fetched(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `211 + s * (1 ±0)`
		//  Estimated: `5252 + s * (1 ±0)`
		// Minimum execution time: 17_995 nanoseconds.
		Weight::from_parts(18_211_000, 0)
			.saturating_add(Weight::from_parts(0, 5252))
			// Standard Error: 1
			.saturating_add(Weight::from_parts(1_168, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
			.saturating_add(Weight::from_parts(0, 1).saturating_mul(s.into()))
	}
	/// Storage: Scheduler Lookup (r:0 w:1)
	/// Proof: Scheduler Lookup (max_values: None, max_size: Some(48), added: 2523, mode: MaxEncodedLen)
	fn service_task_named() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 6_542 nanoseconds.
		Weight::from_parts(6_839_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	fn service_task_periodic() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 5_129 nanoseconds.
		Weight::from_parts(5_321_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	fn execute_dispatch_signed() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_473 nanoseconds.
		Weight::from_parts(2_555_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	fn execute_dispatch_unsigned() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_331 nanoseconds.
		Weight::from_parts(2_494_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 49]`.
	fn schedule(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `148 + s * (177 ±0)`
		//  Estimated: `41438`
		// Minimum execution time: 13_241 nanoseconds.
		Weight::from_parts(16_391_558, 0)
			.saturating_add(Weight::from_parts(0, 41438))
			// Standard Error: 2_240
			.saturating_add(Weight::from_parts(738_603, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// Storage: Scheduler Lookup (r:0 w:1)
	/// Proof: Scheduler Lookup (max_values: None, max_size: Some(48), added: 2523, mode: MaxEncodedLen)
	/// The range of component `s` is `[1, 50]`.
	fn cancel(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `148 + s * (177 ±0)`
		//  Estimated: `41438`
		// Minimum execution time: 17_945 nanoseconds.
		Weight::from_parts(17_048_555, 0)
			.saturating_add(Weight::from_parts(0, 41438))
			// Standard Error: 2_701
			.saturating_add(Weight::from_parts(1_319_435, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Scheduler Lookup (r:1 w:1)
	/// Proof: Scheduler Lookup (max_values: None, max_size: Some(48), added: 2523, mode: MaxEncodedLen)
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 49]`.
	fn schedule_named(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `325 + s * (185 ±0)`
		//  Estimated: `43961`
		// Minimum execution time: 15_926 nanoseconds.
		Weight::from_parts(20_243_204, 0)
			.saturating_add(Weight::from_parts(0, 43961))
			// Standard Error: 3_696
			.saturating_add(Weight::from_parts(765_081, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Scheduler Lookup (r:1 w:1)
	/// Proof: Scheduler Lookup (max_values: None, max_size: Some(48), added: 2523, mode: MaxEncodedLen)
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[1, 50]`.
	fn cancel_named(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `351 + s * (185 ±0)`
		//  Estimated: `43961`
		// Minimum execution time: 18_110 nanoseconds.
		Weight::from_parts(18_859_126, 0)
			.saturating_add(Weight::from_parts(0, 43961))
			// Standard Error: 2_249
			.saturating_add(Weight::from_parts(1_329_477, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
}
