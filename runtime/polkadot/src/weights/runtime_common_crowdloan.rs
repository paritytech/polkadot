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
//! Autogenerated weights for `runtime_common::crowdloan`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-01-11, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm6`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=runtime_common::crowdloan
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/runtime_common_crowdloan.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_common::crowdloan`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_common::crowdloan::WeightInfo for WeightInfo<T> {
	// Storage: Crowdloan Funds (r:1 w:1)
	// Storage: Registrar Paras (r:1 w:1)
	// Storage: Paras ParaLifecycles (r:1 w:0)
	// Storage: Crowdloan NextFundIndex (r:1 w:1)
	fn create() -> Weight {
		// Minimum execution time: 47_510 nanoseconds.
		Weight::from_ref_time(49_718_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Crowdloan Funds (r:1 w:1)
	// Storage: Slots Leases (r:1 w:0)
	// Storage: Auctions AuctionInfo (r:1 w:0)
	// Storage: System Account (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:1)
	// Storage: Crowdloan EndingsCount (r:1 w:0)
	// Storage: Crowdloan NewRaise (r:1 w:1)
	// Storage: unknown [0xd861ea1ebf4800d4b89f4ff787ad79ee96d9a708c85b57da7eb8f9ddeda61291] (r:1 w:1)
	fn contribute() -> Weight {
		// Minimum execution time: 119_065 nanoseconds.
		Weight::from_ref_time(120_198_000)
			.saturating_add(T::DbWeight::get().reads(8))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: Crowdloan Funds (r:1 w:1)
	// Storage: System Account (r:2 w:2)
	// Storage: Balances InactiveIssuance (r:1 w:1)
	// Storage: unknown [0xc85982571aa615c788ef9b2c16f54f25773fd439e8ee1ed2aa3ae43d48e880f0] (r:1 w:1)
	fn withdraw() -> Weight {
		// Minimum execution time: 61_313 nanoseconds.
		Weight::from_ref_time(62_564_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: Skipped Metadata (r:0 w:0)
	/// The range of component `k` is `[0, 1000]`.
	fn refund(k: u32, ) -> Weight {
		// Minimum execution time: 50_174 nanoseconds.
		Weight::from_ref_time(64_924_000)
			// Standard Error: 12_549
			.saturating_add(Weight::from_ref_time(19_083_739).saturating_mul(k.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().reads((2_u64).saturating_mul(k.into())))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(T::DbWeight::get().writes((2_u64).saturating_mul(k.into())))
	}
	// Storage: Crowdloan Funds (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	fn dissolve() -> Weight {
		// Minimum execution time: 38_029 nanoseconds.
		Weight::from_ref_time(39_036_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Crowdloan Funds (r:1 w:1)
	fn edit() -> Weight {
		// Minimum execution time: 24_959 nanoseconds.
		Weight::from_ref_time(26_139_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Crowdloan Funds (r:1 w:0)
	// Storage: unknown [0xd861ea1ebf4800d4b89f4ff787ad79ee96d9a708c85b57da7eb8f9ddeda61291] (r:1 w:1)
	fn add_memo() -> Weight {
		// Minimum execution time: 35_006 nanoseconds.
		Weight::from_ref_time(36_101_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Crowdloan Funds (r:1 w:0)
	// Storage: Crowdloan NewRaise (r:1 w:1)
	fn poke() -> Weight {
		// Minimum execution time: 26_641 nanoseconds.
		Weight::from_ref_time(27_367_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Auctions AuctionInfo (r:1 w:0)
	// Storage: Crowdloan EndingsCount (r:1 w:1)
	// Storage: Crowdloan NewRaise (r:1 w:1)
	// Storage: Crowdloan Funds (r:2 w:0)
	// Storage: Auctions AuctionCounter (r:1 w:0)
	// Storage: Paras ParaLifecycles (r:2 w:0)
	// Storage: Slots Leases (r:2 w:0)
	// Storage: Auctions Winning (r:1 w:1)
	// Storage: Auctions ReservedAmounts (r:2 w:2)
	// Storage: System Account (r:2 w:2)
	/// The range of component `n` is `[2, 100]`.
	fn on_initialize(n: u32, ) -> Weight {
		// Minimum execution time: 105_675 nanoseconds.
		Weight::from_ref_time(14_009_365)
			// Standard Error: 28_305
			.saturating_add(Weight::from_ref_time(40_792_529).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().reads((5_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(T::DbWeight::get().writes((2_u64).saturating_mul(n.into())))
	}
}
