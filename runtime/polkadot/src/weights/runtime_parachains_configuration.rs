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
//! Autogenerated weights for `runtime_parachains::configuration`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-23, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
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
// --pallet=runtime_parachains::configuration
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/runtime_parachains_configuration.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_parachains::configuration`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::configuration::WeightInfo for WeightInfo<T> {
	/// Storage: Configuration PendingConfigs (r:1 w:1)
	/// Proof Skipped: Configuration PendingConfigs (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration ActiveConfig (r:1 w:0)
	/// Proof Skipped: Configuration ActiveConfig (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration BypassConsistencyCheck (r:1 w:0)
	/// Proof Skipped: Configuration BypassConsistencyCheck (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	fn set_config_with_block_number() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3376`
		// Minimum execution time: 11_399 nanoseconds.
		Weight::from_parts(11_874_000, 0)
			.saturating_add(Weight::from_parts(0, 3376))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Configuration PendingConfigs (r:1 w:1)
	/// Proof Skipped: Configuration PendingConfigs (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration ActiveConfig (r:1 w:0)
	/// Proof Skipped: Configuration ActiveConfig (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration BypassConsistencyCheck (r:1 w:0)
	/// Proof Skipped: Configuration BypassConsistencyCheck (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	fn set_config_with_u32() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3376`
		// Minimum execution time: 11_612 nanoseconds.
		Weight::from_parts(12_074_000, 0)
			.saturating_add(Weight::from_parts(0, 3376))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Configuration PendingConfigs (r:1 w:1)
	/// Proof Skipped: Configuration PendingConfigs (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration ActiveConfig (r:1 w:0)
	/// Proof Skipped: Configuration ActiveConfig (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration BypassConsistencyCheck (r:1 w:0)
	/// Proof Skipped: Configuration BypassConsistencyCheck (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	fn set_config_with_option_u32() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3376`
		// Minimum execution time: 11_487 nanoseconds.
		Weight::from_parts(11_881_000, 0)
			.saturating_add(Weight::from_parts(0, 3376))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Configuration PendingConfigs (r:1 w:1)
	/// Proof Skipped: Configuration PendingConfigs (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration ActiveConfig (r:1 w:0)
	/// Proof Skipped: Configuration ActiveConfig (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration BypassConsistencyCheck (r:1 w:0)
	/// Proof Skipped: Configuration BypassConsistencyCheck (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	fn set_config_with_weight() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3376`
		// Minimum execution time: 11_436 nanoseconds.
		Weight::from_parts(11_875_000, 0)
			.saturating_add(Weight::from_parts(0, 3376))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Benchmark Override (r:0 w:0)
	/// Proof Skipped: Benchmark Override (max_values: None, max_size: None, mode: Measured)
	fn set_hrmp_open_request_ttl() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_000_000_000 nanoseconds.
		Weight::from_parts(2_000_000_000_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: Configuration PendingConfigs (r:1 w:1)
	/// Proof Skipped: Configuration PendingConfigs (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration ActiveConfig (r:1 w:0)
	/// Proof Skipped: Configuration ActiveConfig (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Configuration BypassConsistencyCheck (r:1 w:0)
	/// Proof Skipped: Configuration BypassConsistencyCheck (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	fn set_config_with_balance() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3376`
		// Minimum execution time: 11_657 nanoseconds.
		Weight::from_parts(12_045_000, 0)
			.saturating_add(Weight::from_parts(0, 3376))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(1))
	}
}
