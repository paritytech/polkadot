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
//! DATE: 2022-10-03, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm2`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
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
// --output=./runtime/rococo/src/weights

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_scheduler`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_scheduler::WeightInfo for WeightInfo<T> {
	// Storage: Scheduler IncompleteSince (r:1 w:1)
	fn service_agendas_base() -> Weight {
		Weight::from_ref_time(4_700_000 as u64)
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Scheduler Agenda (r:1 w:1)
	/// The range of component `s` is `[0, 50]`.
	fn service_agenda_base(s: u32, ) -> Weight {
		Weight::from_ref_time(3_868_000 as u64)
			// Standard Error: 2_747
			.saturating_add(Weight::from_ref_time(629_992 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	fn service_task_base() -> Weight {
		Weight::from_ref_time(12_316_000 as u64)
	}
	// Storage: Preimage PreimageFor (r:1 w:1)
	// Storage: Preimage StatusFor (r:1 w:1)
	/// The range of component `s` is `[128, 4194304]`.
	fn service_task_fetched(s: u32, ) -> Weight {
		Weight::from_ref_time(24_103_000 as u64)
			// Standard Error: 5
			.saturating_add(Weight::from_ref_time(1_154 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
	// Storage: Scheduler Lookup (r:0 w:1)
	fn service_task_named() -> Weight {
		Weight::from_ref_time(13_408_000 as u64)
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Scheduler Agenda (r:1 w:1)
	fn service_task_periodic() -> Weight {
		Weight::from_ref_time(19_302_000 as u64)
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	fn execute_dispatch_signed() -> Weight {
		Weight::from_ref_time(3_885_000 as u64)
	}
	fn execute_dispatch_unsigned() -> Weight {
		Weight::from_ref_time(4_037_000 as u64)
	}
	// Storage: Scheduler Agenda (r:1 w:1)
	/// The range of component `s` is `[0, 49]`.
	fn schedule(s: u32, ) -> Weight {
		Weight::from_ref_time(16_768_000 as u64)
			// Standard Error: 3_650
			.saturating_add(Weight::from_ref_time(667_428 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Scheduler Agenda (r:1 w:1)
	// Storage: Scheduler Lookup (r:0 w:1)
	/// The range of component `s` is `[1, 50]`.
	fn cancel(s: u32, ) -> Weight {
		Weight::from_ref_time(19_239_000 as u64)
			// Standard Error: 1_456
			.saturating_add(Weight::from_ref_time(578_125 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
	// Storage: Scheduler Lookup (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	/// The range of component `s` is `[0, 49]`.
	fn schedule_named(s: u32, ) -> Weight {
		Weight::from_ref_time(19_065_000 as u64)
			// Standard Error: 4_027
			.saturating_add(Weight::from_ref_time(719_766 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
	// Storage: Scheduler Lookup (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	/// The range of component `s` is `[1, 50]`.
	fn cancel_named(s: u32, ) -> Weight {
		Weight::from_ref_time(20_039_000 as u64)
			// Standard Error: 2_179
			.saturating_add(Weight::from_ref_time(627_335 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
}
