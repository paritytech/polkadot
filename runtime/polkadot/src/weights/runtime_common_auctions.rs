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

//! Autogenerated weights for `runtime_common::auctions`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-04-18, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `runner-yu9ayb4d-project-163-concurrent-0`, CPU: `Intel(R) Xeon(R) CPU @ 2.60GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --no-storage-info
// --no-median-slopes
// --no-min-squares
// --pallet=runtime_common::auctions
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/runtime_common_auctions.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `runtime_common::auctions`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_common::auctions::WeightInfo for WeightInfo<T> {
	/// Storage: Auctions AuctionInfo (r:1 w:1)
	/// Proof: Auctions AuctionInfo (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Auctions AuctionCounter (r:1 w:1)
	/// Proof: Auctions AuctionCounter (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn new_auction() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `4`
		//  Estimated: `2982`
		// Minimum execution time: 13_386_000 picoseconds.
		Weight::from_parts(14_555_000, 0)
			.saturating_add(Weight::from_parts(0, 2982))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Paras ParaLifecycles (r:1 w:0)
	/// Proof Skipped: Paras ParaLifecycles (max_values: None, max_size: None, mode: Measured)
	/// Storage: Auctions AuctionCounter (r:1 w:0)
	/// Proof: Auctions AuctionCounter (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Auctions AuctionInfo (r:1 w:0)
	/// Proof: Auctions AuctionInfo (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Slots Leases (r:1 w:0)
	/// Proof Skipped: Slots Leases (max_values: None, max_size: None, mode: Measured)
	/// Storage: Auctions Winning (r:1 w:1)
	/// Proof: Auctions Winning (max_values: None, max_size: Some(1920), added: 4395, mode: MaxEncodedLen)
	/// Storage: Auctions ReservedAmounts (r:2 w:2)
	/// Proof: Auctions ReservedAmounts (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	fn bid() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `661`
		//  Estimated: `26272`
		// Minimum execution time: 85_753_000 picoseconds.
		Weight::from_parts(89_756_000, 0)
			.saturating_add(Weight::from_parts(0, 26272))
			.saturating_add(T::DbWeight::get().reads(8))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: Auctions AuctionInfo (r:1 w:1)
	/// Proof: Auctions AuctionInfo (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Babe NextRandomness (r:1 w:0)
	/// Proof: Babe NextRandomness (max_values: Some(1), max_size: Some(32), added: 527, mode: MaxEncodedLen)
	/// Storage: Babe EpochStart (r:1 w:0)
	/// Proof: Babe EpochStart (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Auctions AuctionCounter (r:1 w:0)
	/// Proof: Auctions AuctionCounter (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Auctions Winning (r:3600 w:3600)
	/// Proof: Auctions Winning (max_values: None, max_size: Some(1920), added: 4395, mode: MaxEncodedLen)
	/// Storage: Auctions ReservedAmounts (r:37 w:36)
	/// Proof: Auctions ReservedAmounts (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:36 w:36)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Slots Leases (r:7 w:7)
	/// Proof Skipped: Slots Leases (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras ParaLifecycles (r:1 w:1)
	/// Proof Skipped: Paras ParaLifecycles (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Paras ActionsQueue (r:1 w:1)
	/// Proof Skipped: Paras ActionsQueue (max_values: None, max_size: None, mode: Measured)
	/// Storage: Registrar Paras (r:1 w:1)
	/// Proof Skipped: Registrar Paras (max_values: None, max_size: None, mode: Measured)
	fn on_initialize() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `6947662`
		//  Estimated: `50786970`
		// Minimum execution time: 7_382_981_000 picoseconds.
		Weight::from_parts(7_613_582_000, 0)
			.saturating_add(Weight::from_parts(0, 50786970))
			.saturating_add(T::DbWeight::get().reads(3688))
			.saturating_add(T::DbWeight::get().writes(3683))
	}
	/// Storage: Auctions ReservedAmounts (r:37 w:36)
	/// Proof: Auctions ReservedAmounts (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:36 w:36)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Auctions Winning (r:3600 w:3600)
	/// Proof: Auctions Winning (max_values: None, max_size: Some(1920), added: 4395, mode: MaxEncodedLen)
	/// Storage: Auctions AuctionInfo (r:0 w:1)
	/// Proof: Auctions AuctionInfo (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	fn cancel_auction() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `177732`
		//  Estimated: `16012473`
		// Minimum execution time: 5_651_825_000 picoseconds.
		Weight::from_parts(5_791_781_000, 0)
			.saturating_add(Weight::from_parts(0, 16012473))
			.saturating_add(T::DbWeight::get().reads(3673))
			.saturating_add(T::DbWeight::get().writes(3673))
	}
}
