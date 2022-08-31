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
//! Autogenerated weights for `pallet_staking`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-08-19, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm6`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=westend-dev
// --steps=50
// --repeat=20
// --pallet=pallet_staking
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/pallet_staking.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{RefTimeWeight, Weight}};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_staking`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_staking::WeightInfo for WeightInfo<T> {
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking HistoryDepth (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: Staking Payee (r:0 w:1)
	fn bond() -> Weight {
		Weight::from_ref_time(39_056_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(5 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(4 as RefTimeWeight))
	}
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: VoterList ListBags (r:2 w:2)
	fn bond_extra() -> Weight {
		Weight::from_ref_time(70_307_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(8 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(7 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking Nominators (r:1 w:0)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: VoterList ListBags (r:2 w:2)
	fn unbond() -> Weight {
		Weight::from_ref_time(75_717_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(12 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(8 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_update(s: u32, ) -> Weight {
		Weight::from_ref_time(31_047_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(31_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking SlashingSpans (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: Staking Payee (r:0 w:1)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_kill(s: u32, ) -> Weight {
		Weight::from_ref_time(60_033_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(1_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(13 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(11 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking MinValidatorBond (r:1 w:0)
	// Storage: Staking MinCommission (r:1 w:0)
	// Storage: Staking Validators (r:1 w:1)
	// Storage: Staking MaxValidatorsCount (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: VoterList ListNodes (r:1 w:1)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: Staking CounterForValidators (r:1 w:1)
	fn validate() -> Weight {
		Weight::from_ref_time(48_953_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(11 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(5 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	/// The range of component `k` is `[1, 128]`.
	fn kick(k: u32, ) -> Weight {
		Weight::from_ref_time(10_920_000 as RefTimeWeight)
			// Standard Error: 8_000
			.saturating_add(Weight::from_ref_time(8_111_000 as RefTimeWeight).scalar_saturating_mul(k as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((1 as RefTimeWeight).saturating_mul(k as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes((1 as RefTimeWeight).saturating_mul(k as RefTimeWeight)))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking MaxNominatorsCount (r:1 w:0)
	// Storage: Staking Validators (r:2 w:0)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	/// The range of component `n` is `[1, 16]`.
	fn nominate(n: u32, ) -> Weight {
		Weight::from_ref_time(52_622_000 as RefTimeWeight)
			// Standard Error: 11_000
			.saturating_add(Weight::from_ref_time(3_092_000 as RefTimeWeight).scalar_saturating_mul(n as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(12 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((1 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	fn chill() -> Weight {
		Weight::from_ref_time(46_206_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(8 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking Payee (r:0 w:1)
	fn set_payee() -> Weight {
		Weight::from_ref_time(9_480_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking Ledger (r:2 w:2)
	fn set_controller() -> Weight {
		Weight::from_ref_time(16_445_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
	// Storage: Staking ValidatorCount (r:0 w:1)
	fn set_validator_count() -> Weight {
		Weight::from_ref_time(3_236_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking ForceEra (r:0 w:1)
	fn force_no_eras() -> Weight {
		Weight::from_ref_time(3_386_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking ForceEra (r:0 w:1)
	fn force_new_era() -> Weight {
		Weight::from_ref_time(3_324_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking ForceEra (r:0 w:1)
	fn force_new_era_always() -> Weight {
		Weight::from_ref_time(3_340_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking Invulnerables (r:0 w:1)
	/// The range of component `v` is `[0, 1000]`.
	fn set_invulnerables(v: u32, ) -> Weight {
		Weight::from_ref_time(3_676_000 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(10_000 as RefTimeWeight).scalar_saturating_mul(v as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking SlashingSpans (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: Staking Ledger (r:0 w:1)
	// Storage: Staking Payee (r:0 w:1)
	// Storage: Staking SpanSlash (r:0 w:2)
	/// The range of component `s` is `[0, 100]`.
	fn force_unstake(s: u32, ) -> Weight {
		Weight::from_ref_time(57_723_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add(Weight::from_ref_time(894_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(11 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(12 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((1 as RefTimeWeight).saturating_mul(s as RefTimeWeight)))
	}
	// Storage: Staking UnappliedSlashes (r:1 w:1)
	/// The range of component `s` is `[1, 1000]`.
	fn cancel_deferred_slash(s: u32, ) -> Weight {
		Weight::from_ref_time(2_534_473_000 as RefTimeWeight)
			// Standard Error: 172_000
			.saturating_add(Weight::from_ref_time(14_773_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking HistoryDepth (r:1 w:0)
	// Storage: Staking ErasValidatorReward (r:1 w:0)
	// Storage: Staking Bonded (r:2 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking ErasStakersClipped (r:1 w:0)
	// Storage: Staking ErasRewardPoints (r:1 w:0)
	// Storage: Staking ErasValidatorPrefs (r:1 w:0)
	// Storage: Staking Payee (r:2 w:0)
	// Storage: System Account (r:2 w:2)
	/// The range of component `n` is `[1, 64]`.
	fn payout_stakers_dead_controller(n: u32, ) -> Weight {
		Weight::from_ref_time(74_433_000 as RefTimeWeight)
			// Standard Error: 22_000
			.saturating_add(Weight::from_ref_time(24_296_000 as RefTimeWeight).scalar_saturating_mul(n as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(10 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((3 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((1 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
	}
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking HistoryDepth (r:1 w:0)
	// Storage: Staking ErasValidatorReward (r:1 w:0)
	// Storage: Staking Bonded (r:2 w:0)
	// Storage: Staking Ledger (r:2 w:2)
	// Storage: Staking ErasStakersClipped (r:1 w:0)
	// Storage: Staking ErasRewardPoints (r:1 w:0)
	// Storage: Staking ErasValidatorPrefs (r:1 w:0)
	// Storage: Staking Payee (r:2 w:0)
	// Storage: System Account (r:2 w:2)
	// Storage: Balances Locks (r:2 w:2)
	/// The range of component `n` is `[1, 64]`.
	fn payout_stakers_alive_staked(n: u32, ) -> Weight {
		Weight::from_ref_time(83_490_000 as RefTimeWeight)
			// Standard Error: 26_000
			.saturating_add(Weight::from_ref_time(32_049_000 as RefTimeWeight).scalar_saturating_mul(n as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(11 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((5 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((3 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
	}
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: VoterList ListBags (r:2 w:2)
	/// The range of component `l` is `[1, 32]`.
	fn rebond(l: u32, ) -> Weight {
		Weight::from_ref_time(68_977_000 as RefTimeWeight)
			// Standard Error: 13_000
			.saturating_add(Weight::from_ref_time(54_000 as RefTimeWeight).scalar_saturating_mul(l as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(9 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(8 as RefTimeWeight))
	}
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking HistoryDepth (r:1 w:1)
	// Storage: Staking ErasStakersClipped (r:0 w:2)
	// Storage: Staking ErasValidatorPrefs (r:0 w:2)
	// Storage: Staking ErasValidatorReward (r:0 w:1)
	// Storage: Staking ErasRewardPoints (r:0 w:1)
	// Storage: Staking ErasStakers (r:0 w:2)
	// Storage: Staking ErasTotalStake (r:0 w:1)
	// Storage: Staking ErasStartSessionIndex (r:0 w:1)
	/// The range of component `e` is `[1, 100]`.
	fn set_history_depth(e: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 90_000
			.saturating_add(Weight::from_ref_time(22_124_000 as RefTimeWeight).scalar_saturating_mul(e as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((7 as RefTimeWeight).saturating_mul(e as RefTimeWeight)))
	}
	// Storage: System Account (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking SlashingSpans (r:1 w:1)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: Staking Payee (r:0 w:1)
	// Storage: Staking SpanSlash (r:0 w:1)
	/// The range of component `s` is `[1, 100]`.
	fn reap_stash(s: u32, ) -> Weight {
		Weight::from_ref_time(64_117_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add(Weight::from_ref_time(888_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(12 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(12 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((1 as RefTimeWeight).saturating_mul(s as RefTimeWeight)))
	}
	// Storage: VoterList CounterForListNodes (r:1 w:0)
	// Storage: Staking SlashingSpans (r:1 w:0)
	// Storage: VoterList ListBags (r:178 w:0)
	// Storage: VoterList ListNodes (r:101 w:0)
	// Storage: Staking Nominators (r:101 w:0)
	// Storage: Staking Validators (r:2 w:0)
	// Storage: Staking Bonded (r:101 w:0)
	// Storage: Staking Ledger (r:101 w:0)
	// Storage: System BlockWeight (r:1 w:1)
	// Storage: Staking CounterForValidators (r:1 w:0)
	// Storage: Staking ValidatorCount (r:1 w:0)
	// Storage: Staking MinimumValidatorCount (r:1 w:0)
	// Storage: Staking CurrentEra (r:1 w:1)
	// Storage: Staking HistoryDepth (r:1 w:0)
	// Storage: Staking ErasStakersClipped (r:0 w:1)
	// Storage: Staking ErasValidatorPrefs (r:0 w:1)
	// Storage: Staking ErasStakers (r:0 w:1)
	// Storage: Staking ErasTotalStake (r:0 w:1)
	// Storage: Staking ErasStartSessionIndex (r:0 w:1)
	/// The range of component `v` is `[1, 10]`.
	/// The range of component `n` is `[1, 100]`.
	fn new_era(v: u32, n: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 1_326_000
			.saturating_add(Weight::from_ref_time(300_625_000 as RefTimeWeight).scalar_saturating_mul(v as RefTimeWeight))
			// Standard Error: 127_000
			.saturating_add(Weight::from_ref_time(38_619_000 as RefTimeWeight).scalar_saturating_mul(n as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(187 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((5 as RefTimeWeight).saturating_mul(v as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().reads((4 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes((3 as RefTimeWeight).saturating_mul(v as RefTimeWeight)))
	}
	// Storage: VoterList CounterForListNodes (r:1 w:0)
	// Storage: Staking SlashingSpans (r:21 w:0)
	// Storage: VoterList ListBags (r:178 w:0)
	// Storage: VoterList ListNodes (r:1500 w:0)
	// Storage: Staking Nominators (r:1500 w:0)
	// Storage: Staking Validators (r:500 w:0)
	// Storage: Staking Bonded (r:1500 w:0)
	// Storage: Staking Ledger (r:1500 w:0)
	// Storage: System BlockWeight (r:1 w:1)
	/// The range of component `v` is `[500, 1000]`.
	/// The range of component `n` is `[500, 1000]`.
	/// The range of component `s` is `[1, 20]`.
	fn get_npos_voters(v: u32, n: u32, s: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 116_000
			.saturating_add(Weight::from_ref_time(24_599_000 as RefTimeWeight).scalar_saturating_mul(v as RefTimeWeight))
			// Standard Error: 116_000
			.saturating_add(Weight::from_ref_time(22_573_000 as RefTimeWeight).scalar_saturating_mul(n as RefTimeWeight))
			// Standard Error: 2_973_000
			.saturating_add(Weight::from_ref_time(34_144_000 as RefTimeWeight).scalar_saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(181 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((5 as RefTimeWeight).saturating_mul(v as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().reads((4 as RefTimeWeight).saturating_mul(n as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().reads((1 as RefTimeWeight).saturating_mul(s as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking Validators (r:501 w:0)
	// Storage: System BlockWeight (r:1 w:1)
	/// The range of component `v` is `[500, 1000]`.
	fn get_npos_targets(v: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 34_000
			.saturating_add(Weight::from_ref_time(7_766_000 as RefTimeWeight).scalar_saturating_mul(v as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads((1 as RefTimeWeight).saturating_mul(v as RefTimeWeight)))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Staking MinCommission (r:0 w:1)
	// Storage: Staking MinValidatorBond (r:0 w:1)
	// Storage: Staking MaxValidatorsCount (r:0 w:1)
	// Storage: Staking ChillThreshold (r:0 w:1)
	// Storage: Staking MaxNominatorsCount (r:0 w:1)
	// Storage: Staking MinNominatorBond (r:0 w:1)
	fn set_staking_configs_all_set() -> Weight {
		Weight::from_ref_time(6_082_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Staking MinCommission (r:0 w:1)
	// Storage: Staking MinValidatorBond (r:0 w:1)
	// Storage: Staking MaxValidatorsCount (r:0 w:1)
	// Storage: Staking ChillThreshold (r:0 w:1)
	// Storage: Staking MaxNominatorsCount (r:0 w:1)
	// Storage: Staking MinNominatorBond (r:0 w:1)
	fn set_staking_configs_all_remove() -> Weight {
		Weight::from_ref_time(5_821_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking ChillThreshold (r:1 w:0)
	// Storage: Staking MaxNominatorsCount (r:1 w:0)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	fn chill_other() -> Weight {
		Weight::from_ref_time(55_078_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(11 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Staking MinCommission (r:1 w:0)
	// Storage: Staking Validators (r:1 w:1)
	fn force_apply_min_commission() -> Weight {
		Weight::from_ref_time(10_492_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
}
