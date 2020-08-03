// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Weights for the Democracy Pallet
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0-rc5

use frame_support::weights::{Weight, constants::RocksDbWeight as DbWeight};

pub struct WeightInfo;
impl pallet_democracy::WeightInfo for WeightInfo {
	fn propose() -> Weight {
		(49113000 as Weight)
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn second(s: u32, ) -> Weight {
		(42067000 as Weight)
			.saturating_add((220000 as Weight).saturating_mul(s as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn vote_new(r: u32, ) -> Weight {
		(54159000 as Weight)
			.saturating_add((252000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn vote_existing(r: u32, ) -> Weight {
		(54145000 as Weight)
			.saturating_add((262000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn emergency_cancel() -> Weight {
		(31071000 as Weight)
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn external_propose(v: u32, ) -> Weight {
		(14282000 as Weight)
			.saturating_add((109000 as Weight).saturating_mul(v as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn external_propose_majority() -> Weight {
		(3478000 as Weight)
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn external_propose_default() -> Weight {
		(3442000 as Weight)
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn fast_track() -> Weight {
		(30820000 as Weight)
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn veto_external(v: u32, ) -> Weight {
		(30971000 as Weight)
			.saturating_add((184000 as Weight).saturating_mul(v as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn cancel_referendum() -> Weight {
		(20431000 as Weight)
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn cancel_queued(r: u32, ) -> Weight {
		(42438000 as Weight)
			.saturating_add((3284000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn on_initialize_base(r: u32, ) -> Weight {
		(70826000 as Weight)
			.saturating_add((10716000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(6 as Weight))
			.saturating_add(DbWeight::get().reads((2 as Weight).saturating_mul(r as Weight)))
			.saturating_add(DbWeight::get().writes(5 as Weight))
	}
	fn delegate(r: u32, ) -> Weight {
		(72046000 as Weight)
			.saturating_add((7837000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(4 as Weight))
			.saturating_add(DbWeight::get().reads((1 as Weight).saturating_mul(r as Weight)))
			.saturating_add(DbWeight::get().writes(4 as Weight))
			.saturating_add(DbWeight::get().writes((1 as Weight).saturating_mul(r as Weight)))
	}
	fn undelegate(r: u32, ) -> Weight {
		(41028000 as Weight)
			.saturating_add((7810000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().reads((1 as Weight).saturating_mul(r as Weight)))
			.saturating_add(DbWeight::get().writes(2 as Weight))
			.saturating_add(DbWeight::get().writes((1 as Weight).saturating_mul(r as Weight)))
	}
	fn clear_public_proposals() -> Weight {
		(3643000 as Weight)
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn note_preimage(b: u32, ) -> Weight {
		(46629000 as Weight)
			.saturating_add((4000 as Weight).saturating_mul(b as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn note_imminent_preimage(b: u32, ) -> Weight {
		(31147000 as Weight)
			.saturating_add((3000 as Weight).saturating_mul(b as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn reap_preimage(b: u32, ) -> Weight {
		(42848000 as Weight)
			.saturating_add((3000 as Weight).saturating_mul(b as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn unlock_remove(r: u32, ) -> Weight {
		(45333000 as Weight)
			.saturating_add((171000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn unlock_set(r: u32, ) -> Weight {
		(44424000 as Weight)
			.saturating_add((291000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(3 as Weight))
	}
	fn remove_vote(r: u32, ) -> Weight {
		(28250000 as Weight)
			.saturating_add((283000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn remove_other_vote(r: u32, ) -> Weight {
		(28250000 as Weight)
			.saturating_add((283000 as Weight).saturating_mul(r as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
}
