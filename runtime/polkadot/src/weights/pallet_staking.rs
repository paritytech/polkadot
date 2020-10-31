// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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
//! Weights for pallet_staking
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0
//! DATE: 2020-10-29, STEPS: [50, ], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 128

// Executed Command:
// ./target/release/polkadot
// benchmark
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_staking
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --header
// ./file_header.txt
// --output=./runtime/polkadot/src/weights/


#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for pallet_staking.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Trait> pallet_staking::WeightInfo for WeightInfo<T> {
	fn bond() -> Weight {
		(92_188_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(5 as Weight))
			.saturating_add(T::DbWeight::get().writes(4 as Weight))
	}
	fn bond_extra() -> Weight {
		(73_699_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	fn unbond() -> Weight {
		(66_184_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(5 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn withdraw_unbonded_update(s: u32, ) -> Weight {
		(66_448_000 as Weight)
			.saturating_add((29_000 as Weight).saturating_mul(s as Weight))
			.saturating_add(T::DbWeight::get().reads(5 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn withdraw_unbonded_kill(s: u32, ) -> Weight {
		(109_546_000 as Weight)
			.saturating_add((3_844_000 as Weight).saturating_mul(s as Weight))
			.saturating_add(T::DbWeight::get().reads(7 as Weight))
			.saturating_add(T::DbWeight::get().writes(8 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(s as Weight)))
	}
	fn validate() -> Weight {
		(22_866_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	fn nominate(n: u32, ) -> Weight {
		(29_442_000 as Weight)
			.saturating_add((393_000 as Weight).saturating_mul(n as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	fn chill() -> Weight {
		(22_437_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
	fn set_payee() -> Weight {
		(15_252_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn set_controller() -> Weight {
		(33_072_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn set_validator_count() -> Weight {
		(3_200_000 as Weight)
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn force_no_eras() -> Weight {
		(3_567_000 as Weight)
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn force_new_era() -> Weight {
		(3_622_000 as Weight)
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn force_new_era_always() -> Weight {
		(3_593_000 as Weight)
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn set_invulnerables(v: u32, ) -> Weight {
		(3_877_000 as Weight)
			.saturating_add((9_000 as Weight).saturating_mul(v as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn force_unstake(s: u32, ) -> Weight {
		(75_041_000 as Weight)
			.saturating_add((3_835_000 as Weight).saturating_mul(s as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(8 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(s as Weight)))
	}
	fn cancel_deferred_slash(s: u32, ) -> Weight {
		(5_851_773_000 as Weight)
			.saturating_add((34_696_000 as Weight).saturating_mul(s as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn payout_stakers_dead_controller(n: u32, ) -> Weight {
		(132_866_000 as Weight)
			.saturating_add((57_254_000 as Weight).saturating_mul(n as Weight))
			.saturating_add(T::DbWeight::get().reads(11 as Weight))
			.saturating_add(T::DbWeight::get().reads((3 as Weight).saturating_mul(n as Weight)))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(n as Weight)))
	}
	fn payout_stakers_alive_staked(n: u32, ) -> Weight {
		(164_346_000 as Weight)
			.saturating_add((75_078_000 as Weight).saturating_mul(n as Weight))
			.saturating_add(T::DbWeight::get().reads(12 as Weight))
			.saturating_add(T::DbWeight::get().reads((5 as Weight).saturating_mul(n as Weight)))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
			.saturating_add(T::DbWeight::get().writes((3 as Weight).saturating_mul(n as Weight)))
	}
	fn rebond(l: u32, ) -> Weight {
		(45_428_000 as Weight)
			.saturating_add((101_000 as Weight).saturating_mul(l as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(3 as Weight))
	}
	fn set_history_depth(e: u32, ) -> Weight {
		(0 as Weight)
			.saturating_add((36_273_000 as Weight).saturating_mul(e as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as Weight))
			.saturating_add(T::DbWeight::get().writes(4 as Weight))
			.saturating_add(T::DbWeight::get().writes((7 as Weight).saturating_mul(e as Weight)))
	}
	fn reap_stash(s: u32, ) -> Weight {
		(94_854_000 as Weight)
			.saturating_add((3_827_000 as Weight).saturating_mul(s as Weight))
			.saturating_add(T::DbWeight::get().reads(4 as Weight))
			.saturating_add(T::DbWeight::get().writes(8 as Weight))
			.saturating_add(T::DbWeight::get().writes((1 as Weight).saturating_mul(s as Weight)))
	}
	fn new_era(v: u32, n: u32, ) -> Weight {
		(0 as Weight)
			.saturating_add((1_006_982_000 as Weight).saturating_mul(v as Weight))
			.saturating_add((117_552_000 as Weight).saturating_mul(n as Weight))
			.saturating_add(T::DbWeight::get().reads(10 as Weight))
			.saturating_add(T::DbWeight::get().reads((4 as Weight).saturating_mul(v as Weight)))
			.saturating_add(T::DbWeight::get().reads((3 as Weight).saturating_mul(n as Weight)))
			.saturating_add(T::DbWeight::get().writes(8 as Weight))
			.saturating_add(T::DbWeight::get().writes((3 as Weight).saturating_mul(v as Weight)))
	}
	fn submit_solution_better(v: u32, n: u32, a: u32, w: u32, ) -> Weight {
		(0 as Weight)
			.saturating_add((914_000 as Weight).saturating_mul(v as Weight))
			.saturating_add((378_000 as Weight).saturating_mul(n as Weight))
			.saturating_add((96_533_000 as Weight).saturating_mul(a as Weight))
			.saturating_add((7_530_000 as Weight).saturating_mul(w as Weight))
			.saturating_add(T::DbWeight::get().reads(6 as Weight))
			.saturating_add(T::DbWeight::get().reads((4 as Weight).saturating_mul(a as Weight)))
			.saturating_add(T::DbWeight::get().reads((1 as Weight).saturating_mul(w as Weight)))
			.saturating_add(T::DbWeight::get().writes(2 as Weight))
	}
}
