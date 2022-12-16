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
//! Autogenerated weights for `pallet_ranked_collective`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-12-15, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm4`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_ranked_collective
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_ranked_collective`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_ranked_collective::WeightInfo for WeightInfo<T> {
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:0 w:1)
	// Storage: FellowshipCollective IdToIndex (r:0 w:1)
	fn add_member() -> Weight {
		// Minimum execution time: 20_783 nanoseconds.
		Weight::from_ref_time(21_534_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IdToIndex (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:1 w:1)
	/// The range of component `r` is `[0, 10]`.
	fn remove_member(r: u32, ) -> Weight {
		// Minimum execution time: 32_177 nanoseconds.
		Weight::from_ref_time(34_744_578)
			// Standard Error: 21_214
			.saturating_add(Weight::from_ref_time(10_025_635).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().reads((3_u64).saturating_mul(r.into())))
			.saturating_add(T::DbWeight::get().writes(4))
			.saturating_add(T::DbWeight::get().writes((3_u64).saturating_mul(r.into())))
	}
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:0 w:1)
	// Storage: FellowshipCollective IdToIndex (r:0 w:1)
	/// The range of component `r` is `[0, 10]`.
	fn promote_member(r: u32, ) -> Weight {
		// Minimum execution time: 23_280 nanoseconds.
		Weight::from_ref_time(24_473_719)
			// Standard Error: 5_281
			.saturating_add(Weight::from_ref_time(464_452).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IdToIndex (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:1 w:1)
	/// The range of component `r` is `[0, 10]`.
	fn demote_member(r: u32, ) -> Weight {
		// Minimum execution time: 32_351 nanoseconds.
		Weight::from_ref_time(34_734_099)
			// Standard Error: 15_729
			.saturating_add(Weight::from_ref_time(644_092).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:0)
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective Voting (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn vote() -> Weight {
		// Minimum execution time: 47_216 nanoseconds.
		Weight::from_ref_time(47_698_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:0)
	// Storage: FellowshipCollective VotingCleanup (r:1 w:0)
	// Storage: FellowshipCollective Voting (r:0 w:2)
	/// The range of component `n` is `[0, 100]`.
	fn cleanup_poll(n: u32, ) -> Weight {
		// Minimum execution time: 15_139 nanoseconds.
		Weight::from_ref_time(19_621_273)
			// Standard Error: 2_430
			.saturating_add(Weight::from_ref_time(906_348).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(n.into())))
	}
}
