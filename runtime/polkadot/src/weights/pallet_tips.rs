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
//! Autogenerated weights for `pallet_tips`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-28, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
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
// --pallet=pallet_tips
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

/// Weight functions for `pallet_tips`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_tips::WeightInfo for WeightInfo<T> {
	/// Storage: Tips Reasons (r:1 w:1)
	/// Proof Skipped: Tips Reasons (max_values: None, max_size: None, mode: Measured)
	/// Storage: Tips Tips (r:1 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// The range of component `r` is `[0, 16384]`.
	fn report_awesome(r: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `4`
		//  Estimated: `4958`
		// Minimum execution time: 24_717 nanoseconds.
		Weight::from_ref_time(25_747_782)
			.saturating_add(Weight::from_proof_size(4958))
			// Standard Error: 4
			.saturating_add(Weight::from_ref_time(1_823).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Tips Tips (r:1 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// Storage: Tips Reasons (r:0 w:1)
	/// Proof Skipped: Tips Reasons (max_values: None, max_size: None, mode: Measured)
	fn retract_tip() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `253`
		//  Estimated: `2981`
		// Minimum execution time: 23_963 nanoseconds.
		Weight::from_ref_time(24_323_000)
			.saturating_add(Weight::from_proof_size(2981))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: PhragmenElection Members (r:1 w:0)
	/// Proof Skipped: PhragmenElection Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Tips Reasons (r:1 w:1)
	/// Proof Skipped: Tips Reasons (max_values: None, max_size: None, mode: Measured)
	/// Storage: Tips Tips (r:0 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// The range of component `r` is `[0, 16384]`.
	/// The range of component `t` is `[1, 13]`.
	fn tip_new(r: u32, t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `106 + t * (64 ±0)`
		//  Estimated: `3288 + t * (192 ±0)`
		// Minimum execution time: 20_330 nanoseconds.
		Weight::from_ref_time(19_770_627)
			.saturating_add(Weight::from_proof_size(3288))
			// Standard Error: 5
			.saturating_add(Weight::from_ref_time(1_722).saturating_mul(r.into()))
			// Standard Error: 6_711
			.saturating_add(Weight::from_ref_time(133_401).saturating_mul(t.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
			.saturating_add(Weight::from_proof_size(192).saturating_mul(t.into()))
	}
	/// Storage: PhragmenElection Members (r:1 w:0)
	/// Proof Skipped: PhragmenElection Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Tips Tips (r:1 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// The range of component `t` is `[1, 13]`.
	fn tip(t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `359 + t * (112 ±0)`
		//  Estimated: `3688 + t * (224 ±0)`
		// Minimum execution time: 15_707 nanoseconds.
		Weight::from_ref_time(15_992_893)
			.saturating_add(Weight::from_proof_size(3688))
			// Standard Error: 2_102
			.saturating_add(Weight::from_ref_time(122_754).saturating_mul(t.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(Weight::from_proof_size(224).saturating_mul(t.into()))
	}
	/// Storage: Tips Tips (r:1 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// Storage: PhragmenElection Members (r:1 w:0)
	/// Proof Skipped: PhragmenElection Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Tips Reasons (r:0 w:1)
	/// Proof Skipped: Tips Reasons (max_values: None, max_size: None, mode: Measured)
	/// The range of component `t` is `[1, 13]`.
	fn close_tip(t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `398 + t * (112 ±0)`
		//  Estimated: `6740 + t * (336 ±0)`
		// Minimum execution time: 44_350 nanoseconds.
		Weight::from_ref_time(46_263_953)
			.saturating_add(Weight::from_proof_size(6740))
			// Standard Error: 8_644
			.saturating_add(Weight::from_ref_time(147_757).saturating_mul(t.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_proof_size(336).saturating_mul(t.into()))
	}
	/// Storage: Tips Tips (r:1 w:1)
	/// Proof Skipped: Tips Tips (max_values: None, max_size: None, mode: Measured)
	/// Storage: Tips Reasons (r:0 w:1)
	/// Proof Skipped: Tips Reasons (max_values: None, max_size: None, mode: Measured)
	/// The range of component `t` is `[1, 13]`.
	fn slash_tip(t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `301`
		//  Estimated: `3077`
		// Minimum execution time: 15_401 nanoseconds.
		Weight::from_ref_time(16_069_268)
			.saturating_add(Weight::from_proof_size(3077))
			// Standard Error: 2_069
			.saturating_add(Weight::from_ref_time(13_562).saturating_mul(t.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
}
