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
//! Autogenerated weights for `pallet_bounties`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-27, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
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
// --pallet=pallet_bounties
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

/// Weight functions for `pallet_bounties`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_bounties::WeightInfo for WeightInfo<T> {
	/// Storage: Bounties BountyCount (r:1 w:1)
	/// Proof: Bounties BountyCount (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Bounties BountyDescriptions (r:0 w:1)
	/// Proof: Bounties BountyDescriptions (max_values: None, max_size: Some(16400), added: 18875, mode: MaxEncodedLen)
	/// Storage: Bounties Bounties (r:0 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// The range of component `d` is `[0, 16384]`.
	fn propose_bounty(d: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `175`
		//  Estimated: `3102`
		// Minimum execution time: 25_384 nanoseconds.
		Weight::from_ref_time(26_855_698)
			.saturating_add(Weight::from_proof_size(3102))
			// Standard Error: 5
			.saturating_add(Weight::from_ref_time(759).saturating_mul(d.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: Bounties BountyApprovals (r:1 w:1)
	/// Proof: Bounties BountyApprovals (max_values: Some(1), max_size: Some(402), added: 897, mode: MaxEncodedLen)
	fn approve_bounty() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `267`
		//  Estimated: `3549`
		// Minimum execution time: 12_031 nanoseconds.
		Weight::from_ref_time(12_313_000)
			.saturating_add(Weight::from_proof_size(3549))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	fn propose_curator() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `287`
		//  Estimated: `2652`
		// Minimum execution time: 10_431 nanoseconds.
		Weight::from_ref_time(10_741_000)
			.saturating_add(Weight::from_proof_size(2652))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	fn unassign_curator() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `5255`
		// Minimum execution time: 35_648 nanoseconds.
		Weight::from_ref_time(36_567_000)
			.saturating_add(Weight::from_proof_size(5255))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	fn accept_curator() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `491`
		//  Estimated: `5255`
		// Minimum execution time: 23_997 nanoseconds.
		Weight::from_ref_time(24_334_000)
			.saturating_add(Weight::from_proof_size(5255))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: ChildBounties ParentChildBounties (r:1 w:0)
	/// Proof: ChildBounties ParentChildBounties (max_values: None, max_size: Some(16), added: 2491, mode: MaxEncodedLen)
	fn award_bounty() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `437`
		//  Estimated: `5143`
		// Minimum execution time: 20_452 nanoseconds.
		Weight::from_ref_time(20_827_000)
			.saturating_add(Weight::from_proof_size(5143))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: System Account (r:3 w:3)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: ChildBounties ChildrenCuratorFees (r:1 w:1)
	/// Proof: ChildBounties ChildrenCuratorFees (max_values: None, max_size: Some(28), added: 2503, mode: MaxEncodedLen)
	/// Storage: Bounties BountyDescriptions (r:0 w:1)
	/// Proof: Bounties BountyDescriptions (max_values: None, max_size: Some(16400), added: 18875, mode: MaxEncodedLen)
	fn claim_bounty() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `865`
		//  Estimated: `12964`
		// Minimum execution time: 76_289 nanoseconds.
		Weight::from_ref_time(77_242_000)
			.saturating_add(Weight::from_proof_size(12964))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(6))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: ChildBounties ParentChildBounties (r:1 w:0)
	/// Proof: ChildBounties ParentChildBounties (max_values: None, max_size: Some(16), added: 2491, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Bounties BountyDescriptions (r:0 w:1)
	/// Proof: Bounties BountyDescriptions (max_values: None, max_size: Some(16400), added: 18875, mode: MaxEncodedLen)
	fn close_bounty_proposed() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `513`
		//  Estimated: `7746`
		// Minimum execution time: 42_076 nanoseconds.
		Weight::from_ref_time(42_491_000)
			.saturating_add(Weight::from_proof_size(7746))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: ChildBounties ParentChildBounties (r:1 w:0)
	/// Proof: ChildBounties ParentChildBounties (max_values: None, max_size: Some(16), added: 2491, mode: MaxEncodedLen)
	/// Storage: System Account (r:2 w:2)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Bounties BountyDescriptions (r:0 w:1)
	/// Proof: Bounties BountyDescriptions (max_values: None, max_size: Some(16400), added: 18875, mode: MaxEncodedLen)
	fn close_bounty_active() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `781`
		//  Estimated: `10349`
		// Minimum execution time: 51_981 nanoseconds.
		Weight::from_ref_time(52_413_000)
			.saturating_add(Weight::from_proof_size(10349))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: Bounties Bounties (r:1 w:1)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	fn extend_bounty_expiry() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `323`
		//  Estimated: `2652`
		// Minimum execution time: 15_555 nanoseconds.
		Weight::from_ref_time(15_946_000)
			.saturating_add(Weight::from_proof_size(2652))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Bounties BountyApprovals (r:1 w:1)
	/// Proof: Bounties BountyApprovals (max_values: Some(1), max_size: Some(402), added: 897, mode: MaxEncodedLen)
	/// Storage: Bounties Bounties (r:100 w:100)
	/// Proof: Bounties Bounties (max_values: None, max_size: Some(177), added: 2652, mode: MaxEncodedLen)
	/// Storage: System Account (r:200 w:200)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// The range of component `b` is `[0, 100]`.
	fn spend_funds(b: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0 + b * (360 ±0)`
		//  Estimated: `897 + b * (7858 ±0)`
		// Minimum execution time: 5_282 nanoseconds.
		Weight::from_ref_time(1_893_501)
			.saturating_add(Weight::from_proof_size(897))
			// Standard Error: 29_881
			.saturating_add(Weight::from_ref_time(34_151_121).saturating_mul(b.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().reads((3_u64).saturating_mul(b.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((3_u64).saturating_mul(b.into())))
			.saturating_add(Weight::from_parts(0, 7858).saturating_mul(b.into()))
	}
}
