// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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
//! Autogenerated weights for `pallet_collective`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-08-27, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 128

// Executed Command:
// target/release/polkadot
// benchmark
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_collective
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/


#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{RefTimeWeight, Weight}};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_collective`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_collective::WeightInfo for WeightInfo<T> {
	// Storage: Instance1Collective Members (r:1 w:1)
	// Storage: Instance1Collective Proposals (r:1 w:0)
	// Storage: Instance1Collective Voting (r:100 w:100)
	// Storage: Instance1Collective Prime (r:0 w:1)
	fn set_members(m: u32, n: u32, p: u32, ) -> Weight {
		(0 as RefTimeWeight)
			// Standard Error: 6_000
			.saturating_add((14_248_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 6_000
			.saturating_add((320_000 as RefTimeWeight).scalar_saturating_mul(n as Weight))
			// Standard Error: 6_000
			.saturating_add((19_166_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((1 as RefTimeWeight).saturating_mul(p as Weight)))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((1 as RefTimeWeight).saturating_mul(p as Weight)))
	}
	// Storage: Instance1Collective Members (r:1 w:0)
	fn execute(b: u32, m: u32, ) -> Weight {
		(21_101_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((3_000 as RefTimeWeight).scalar_saturating_mul(b as Weight))
			// Standard Error: 0
			.saturating_add((83_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
	}
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective ProposalOf (r:1 w:0)
	fn propose_execute(b: u32, m: u32, ) -> Weight {
		(25_378_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((3_000 as RefTimeWeight).scalar_saturating_mul(b as Weight))
			// Standard Error: 0
			.saturating_add((163_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
	}
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective ProposalOf (r:1 w:1)
	// Storage: Instance1Collective Proposals (r:1 w:1)
	// Storage: Instance1Collective ProposalCount (r:1 w:1)
	// Storage: Instance1Collective Voting (r:0 w:1)
	fn propose_proposed(b: u32, m: u32, p: u32, ) -> Weight {
		(40_063_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((4_000 as RefTimeWeight).scalar_saturating_mul(b as Weight))
			// Standard Error: 0
			.saturating_add((88_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 0
			.saturating_add((373_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(4 as RefTimeWeight))
	}
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective Voting (r:1 w:1)
	fn vote(m: u32, ) -> Weight {
		(31_307_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((196_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Instance1Collective Voting (r:1 w:1)
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective Proposals (r:1 w:1)
	// Storage: Instance1Collective ProposalOf (r:0 w:1)
	fn close_early_disapproved(m: u32, p: u32, ) -> Weight {
		(39_515_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((165_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 0
			.saturating_add((343_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Instance1Collective Voting (r:1 w:1)
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective ProposalOf (r:1 w:1)
	// Storage: Instance1Collective Proposals (r:1 w:1)
	fn close_early_approved(b: u32, m: u32, p: u32, ) -> Weight {
		(54_757_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((3_000 as RefTimeWeight).scalar_saturating_mul(b as Weight))
			// Standard Error: 0
			.saturating_add((163_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 0
			.saturating_add((340_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Instance1Collective Voting (r:1 w:1)
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective Prime (r:1 w:0)
	// Storage: Instance1Collective Proposals (r:1 w:1)
	// Storage: Instance1Collective ProposalOf (r:0 w:1)
	fn close_disapproved(m: u32, p: u32, ) -> Weight {
		(43_851_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((167_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 0
			.saturating_add((344_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Instance1Collective Voting (r:1 w:1)
	// Storage: Instance1Collective Members (r:1 w:0)
	// Storage: Instance1Collective Prime (r:1 w:0)
	// Storage: Instance1Collective ProposalOf (r:1 w:1)
	// Storage: Instance1Collective Proposals (r:1 w:1)
	fn close_approved(b: u32, m: u32, p: u32, ) -> Weight {
		(57_946_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((3_000 as RefTimeWeight).scalar_saturating_mul(b as Weight))
			// Standard Error: 0
			.saturating_add((168_000 as RefTimeWeight).scalar_saturating_mul(m as Weight))
			// Standard Error: 0
			.saturating_add((344_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(5 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Instance1Collective Proposals (r:1 w:1)
	// Storage: Instance1Collective Voting (r:0 w:1)
	// Storage: Instance1Collective ProposalOf (r:0 w:1)
	fn disapprove_proposal(p: u32, ) -> Weight {
		(24_228_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add((348_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
}
