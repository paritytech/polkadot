// Copyright (C) Parity Technologies (UK) Ltd.
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
//! DATE: 2023-05-26, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("rococo-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=rococo-dev
// --steps=50
// --repeat=20
// --pallet=pallet_scheduler
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/rococo/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `pallet_scheduler`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_scheduler::WeightInfo for WeightInfo<T> {
	/// Storage: Scheduler IncompleteSince (r:1 w:1)
	/// Proof: Scheduler IncompleteSince (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn service_agendas_base() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `69`
		//  Estimated: `1489`
		// Minimum execution time: 4_741_000 picoseconds.
		Weight::from_parts(4_939_000, 0)
			.saturating_add(Weight::from_parts(0, 1489))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 50]`.
	fn service_agenda_base(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `116 + s * (177 ±0)`
		//  Estimated: `42428`
		// Minimum execution time: 4_504_000 picoseconds.
		Weight::from_parts(7_569_333, 0)
			.saturating_add(Weight::from_parts(0, 42428))
			// Standard Error: 1_818
			.saturating_add(Weight::from_parts(771_180, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	fn service_task_base() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 5_709_000 picoseconds.
		Weight::from_parts(5_929_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: Preimage PreimageFor (r:1 w:1)
	/// Proof: Preimage PreimageFor (max_values: None, max_size: Some(4194344), added: 4196819, mode: Measured)
	/// Storage: Preimage StatusFor (r:1 w:1)
	/// Proof: Preimage StatusFor (max_values: None, max_size: Some(91), added: 2566, mode: MaxEncodedLen)
	/// The range of component `s` is `[128, 4194304]`.
	fn service_task_fetched(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `251 + s * (1 ±0)`
		//  Estimated: `3716 + s * (1 ±0)`
		// Minimum execution time: 20_710_000 picoseconds.
		Weight::from_parts(20_918_000, 0)
			.saturating_add(Weight::from_parts(0, 3716))
			// Standard Error: 9
			.saturating_add(Weight::from_parts(1_257, 0).saturating_mul(s.into()))
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
		// Minimum execution time: 7_262_000 picoseconds.
		Weight::from_parts(7_412_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	fn service_task_periodic() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 5_774_000 picoseconds.
		Weight::from_parts(5_887_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	fn execute_dispatch_signed() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_777_000 picoseconds.
		Weight::from_parts(2_865_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	fn execute_dispatch_unsigned() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_739_000 picoseconds.
		Weight::from_parts(2_827_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: Scheduler Agenda (r:1 w:1)
	/// Proof: Scheduler Agenda (max_values: None, max_size: Some(38963), added: 41438, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 49]`.
	fn schedule(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `116 + s * (177 ±0)`
		//  Estimated: `42428`
		// Minimum execution time: 14_788_000 picoseconds.
		Weight::from_parts(17_705_748, 0)
			.saturating_add(Weight::from_parts(0, 42428))
			// Standard Error: 1_703
			.saturating_add(Weight::from_parts(760_991, 0).saturating_mul(s.into()))
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
		//  Measured:  `116 + s * (177 ±0)`
		//  Estimated: `42428`
		// Minimum execution time: 18_716_000 picoseconds.
		Weight::from_parts(18_220_022, 0)
			.saturating_add(Weight::from_parts(0, 42428))
			// Standard Error: 1_508
			.saturating_add(Weight::from_parts(1_357_835, 0).saturating_mul(s.into()))
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
		//  Measured:  `293 + s * (185 ±0)`
		//  Estimated: `42428`
		// Minimum execution time: 17_719_000 picoseconds.
		Weight::from_parts(21_657_806, 0)
			.saturating_add(Weight::from_parts(0, 42428))
			// Standard Error: 2_645
			.saturating_add(Weight::from_parts(794_184, 0).saturating_mul(s.into()))
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
		//  Measured:  `319 + s * (185 ±0)`
		//  Estimated: `42428`
		// Minimum execution time: 20_225_000 picoseconds.
		Weight::from_parts(20_494_405, 0)
			.saturating_add(Weight::from_parts(0, 42428))
			// Standard Error: 1_890
			.saturating_add(Weight::from_parts(1_379_025, 0).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
}
