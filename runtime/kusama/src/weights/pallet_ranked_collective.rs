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
//! DATE: 2023-01-11, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
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
		// Minimum execution time: 20_345 nanoseconds.
		Weight::from_ref_time(21_390_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IdToIndex (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:1 w:1)
	/// The range of component `r` is `[0, 10]`.
	fn remove_member(r: u32, ) -> Weight {
		// Minimum execution time: 31_490 nanoseconds.
		Weight::from_ref_time(34_193_442)
			// Standard Error: 21_069
			.saturating_add(Weight::from_ref_time(9_818_196).saturating_mul(r.into()))
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
		// Minimum execution time: 23_185 nanoseconds.
		Weight::from_ref_time(24_111_583)
			// Standard Error: 5_139
			.saturating_add(Weight::from_ref_time(435_094).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:1)
	// Storage: FellowshipCollective IdToIndex (r:1 w:1)
	// Storage: FellowshipCollective IndexToId (r:1 w:1)
	/// The range of component `r` is `[0, 10]`.
	fn demote_member(r: u32, ) -> Weight {
		// Minimum execution time: 31_250 nanoseconds.
		Weight::from_ref_time(34_534_455)
			// Standard Error: 16_714
			.saturating_add(Weight::from_ref_time(572_989).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipCollective Members (r:1 w:0)
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective Voting (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn vote() -> Weight {
		// Minimum execution time: 47_463 nanoseconds.
		Weight::from_ref_time(48_109_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:0)
	// Storage: FellowshipCollective VotingCleanup (r:1 w:0)
	// Storage: FellowshipCollective Voting (r:0 w:2)
	/// The range of component `n` is `[0, 100]`.
	fn cleanup_poll(n: u32, ) -> Weight {
		// Minimum execution time: 15_195 nanoseconds.
		Weight::from_ref_time(19_545_466)
			// Standard Error: 1_861
			.saturating_add(Weight::from_ref_time(908_694).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(n.into())))
	}
}
