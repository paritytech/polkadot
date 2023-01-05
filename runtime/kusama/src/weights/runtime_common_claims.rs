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
//! Autogenerated weights for `runtime_common::claims`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-11-16, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=runtime_common::claims
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/runtime_common_claims.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_common::claims`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_common::claims::WeightInfo for WeightInfo<T> {
	// Storage: Claims Claims (r:1 w:1)
	// Storage: Claims Signing (r:1 w:1)
	// Storage: Claims Total (r:1 w:1)
	// Storage: Claims Vesting (r:1 w:1)
	// Storage: Vesting Vesting (r:1 w:1)
	// Storage: System Account (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	fn claim() -> Weight {
		// Minimum execution time: 144_357 nanoseconds.
		Weight::from_ref_time(146_222_000 as u64)
			.saturating_add(T::DbWeight::get().reads(7 as u64))
			.saturating_add(T::DbWeight::get().writes(6 as u64))
	}
	// Storage: Claims Total (r:1 w:1)
	// Storage: Claims Vesting (r:0 w:1)
	// Storage: Claims Claims (r:0 w:1)
	// Storage: Claims Signing (r:0 w:1)
	fn mint_claim() -> Weight {
		// Minimum execution time: 11_388 nanoseconds.
		Weight::from_ref_time(11_833_000 as u64)
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(4 as u64))
	}
	// Storage: Claims Claims (r:1 w:1)
	// Storage: Claims Signing (r:1 w:1)
	// Storage: Claims Total (r:1 w:1)
	// Storage: Claims Vesting (r:1 w:1)
	// Storage: Vesting Vesting (r:1 w:1)
	// Storage: System Account (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	fn claim_attest() -> Weight {
		// Minimum execution time: 148_256 nanoseconds.
		Weight::from_ref_time(153_700_000 as u64)
			.saturating_add(T::DbWeight::get().reads(7 as u64))
			.saturating_add(T::DbWeight::get().writes(6 as u64))
	}
	// Storage: Claims Preclaims (r:1 w:1)
	// Storage: Claims Signing (r:1 w:1)
	// Storage: Claims Claims (r:1 w:1)
	// Storage: Claims Total (r:1 w:1)
	// Storage: Claims Vesting (r:1 w:1)
	// Storage: Vesting Vesting (r:1 w:1)
	// Storage: System Account (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	fn attest() -> Weight {
		// Minimum execution time: 68_246 nanoseconds.
		Weight::from_ref_time(70_821_000 as u64)
			.saturating_add(T::DbWeight::get().reads(8 as u64))
			.saturating_add(T::DbWeight::get().writes(7 as u64))
	}
	// Storage: Claims Claims (r:1 w:2)
	// Storage: Claims Vesting (r:1 w:2)
	// Storage: Claims Signing (r:1 w:2)
	// Storage: Claims Preclaims (r:1 w:1)
	fn move_claim() -> Weight {
		// Minimum execution time: 21_200 nanoseconds.
		Weight::from_ref_time(21_813_000 as u64)
			.saturating_add(T::DbWeight::get().reads(4 as u64))
			.saturating_add(T::DbWeight::get().writes(7 as u64))
	}
}
