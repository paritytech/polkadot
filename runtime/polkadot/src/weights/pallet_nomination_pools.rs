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
//! Autogenerated weights for `pallet_nomination_pools`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-01-23, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `runner-b3zmxxc-project-163-concurrent-0`, CPU: `Intel(R) Xeon(R) CPU @ 2.60GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_nomination_pools
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

/// Weight functions for `pallet_nomination_pools`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_nomination_pools::WeightInfo for WeightInfo<T> {
	// Storage: NominationPools MinJoinBond (r:1 w:0)
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: System Account (r:2 w:1)
	// Storage: NominationPools MaxPoolMembersPerPool (r:1 w:0)
	// Storage: NominationPools MaxPoolMembers (r:1 w:0)
	// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: VoterList ListBags (r:2 w:2)
	fn join() -> Weight {
		// Minimum execution time: 154_018 nanoseconds.
		Weight::from_ref_time(158_743_000)
			.saturating_add(T::DbWeight::get().reads(17))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: System Account (r:3 w:2)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: VoterList ListBags (r:2 w:2)
	fn bond_extra_transfer() -> Weight {
		// Minimum execution time: 154_682 nanoseconds.
		Weight::from_ref_time(158_809_000)
			.saturating_add(T::DbWeight::get().reads(14))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: System Account (r:3 w:3)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: VoterList ListNodes (r:2 w:2)
	// Storage: VoterList ListBags (r:2 w:2)
	fn bond_extra_other() -> Weight {
		// Minimum execution time: 160_860 nanoseconds.
		Weight::from_ref_time(165_279_000)
			.saturating_add(T::DbWeight::get().reads(13))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	fn claim_payout() -> Weight {
		// Minimum execution time: 58_198 nanoseconds.
		Weight::from_ref_time(59_465_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: System Account (r:2 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:0)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: VoterList ListNodes (r:3 w:3)
	// Storage: VoterList ListBags (r:2 w:2)
	// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	// Storage: NominationPools CounterForSubPoolsStorage (r:1 w:1)
	fn unbond() -> Weight {
		// Minimum execution time: 160_938 nanoseconds.
		Weight::from_ref_time(165_921_000)
			.saturating_add(T::DbWeight::get().reads(18))
			.saturating_add(T::DbWeight::get().writes(13))
	}
	// Storage: NominationPools BondedPools (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	/// The range of component `s` is `[0, 100]`.
	fn pool_withdraw_unbonded(s: u32, ) -> Weight {
		// Minimum execution time: 56_444 nanoseconds.
		Weight::from_ref_time(59_542_905)
			// Standard Error: 1_668
			.saturating_add(Weight::from_ref_time(65_825).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_update(s: u32, ) -> Weight {
		// Minimum execution time: 103_876 nanoseconds.
		Weight::from_ref_time(108_076_686)
			// Standard Error: 2_685
			.saturating_add(Weight::from_ref_time(97_226).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(9))
			.saturating_add(T::DbWeight::get().writes(7))
	}
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking SlashingSpans (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:0)
	// Storage: System Account (r:2 w:2)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	// Storage: NominationPools ReversePoolIdLookup (r:1 w:1)
	// Storage: NominationPools CounterForReversePoolIdLookup (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: NominationPools CounterForRewardPools (r:1 w:1)
	// Storage: NominationPools CounterForSubPoolsStorage (r:1 w:1)
	// Storage: NominationPools Metadata (r:1 w:1)
	// Storage: NominationPools CounterForBondedPools (r:1 w:1)
	// Storage: Staking Payee (r:0 w:1)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_kill(_s: u32, ) -> Weight {
		// Minimum execution time: 158_555 nanoseconds.
		Weight::from_ref_time(165_982_647)
			.saturating_add(T::DbWeight::get().reads(20))
			.saturating_add(T::DbWeight::get().writes(17))
	}
	// Storage: NominationPools LastPoolId (r:1 w:1)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: NominationPools MinCreateBond (r:1 w:0)
	// Storage: NominationPools MinJoinBond (r:1 w:0)
	// Storage: NominationPools MaxPools (r:1 w:0)
	// Storage: NominationPools CounterForBondedPools (r:1 w:1)
	// Storage: NominationPools PoolMembers (r:1 w:1)
	// Storage: NominationPools MaxPoolMembersPerPool (r:1 w:0)
	// Storage: NominationPools MaxPoolMembers (r:1 w:0)
	// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	// Storage: System Account (r:2 w:2)
	// Storage: Staking Bonded (r:1 w:1)
	// Storage: Staking Ledger (r:1 w:1)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: Balances Locks (r:1 w:1)
	// Storage: NominationPools RewardPools (r:1 w:1)
	// Storage: NominationPools CounterForRewardPools (r:1 w:1)
	// Storage: NominationPools ReversePoolIdLookup (r:1 w:1)
	// Storage: NominationPools CounterForReversePoolIdLookup (r:1 w:1)
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: Staking Payee (r:0 w:1)
	fn create() -> Weight {
		// Minimum execution time: 135_955 nanoseconds.
		Weight::from_ref_time(138_419_000)
			.saturating_add(T::DbWeight::get().reads(21))
			.saturating_add(T::DbWeight::get().writes(15))
	}
	// Storage: NominationPools BondedPools (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking MinNominatorBond (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking MaxNominatorsCount (r:1 w:0)
	// Storage: Staking Validators (r:2 w:0)
	// Storage: Staking CurrentEra (r:1 w:0)
	// Storage: VoterList ListNodes (r:1 w:1)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	/// The range of component `n` is `[1, 16]`.
	fn nominate(n: u32, ) -> Weight {
		// Minimum execution time: 65_651 nanoseconds.
		Weight::from_ref_time(66_913_897)
			// Standard Error: 8_660
			.saturating_add(Weight::from_ref_time(1_169_701).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(12))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: NominationPools BondedPools (r:1 w:1)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:0)
	fn set_state() -> Weight {
		// Minimum execution time: 38_320 nanoseconds.
		Weight::from_ref_time(39_484_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: NominationPools BondedPools (r:1 w:0)
	// Storage: NominationPools Metadata (r:1 w:1)
	// Storage: NominationPools CounterForMetadata (r:1 w:1)
	/// The range of component `n` is `[1, 256]`.
	fn set_metadata(n: u32, ) -> Weight {
		// Minimum execution time: 14_423 nanoseconds.
		Weight::from_ref_time(15_213_319)
			// Standard Error: 176
			.saturating_add(Weight::from_ref_time(2_669).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: NominationPools MinJoinBond (r:0 w:1)
	// Storage: NominationPools MaxPoolMembers (r:0 w:1)
	// Storage: NominationPools MaxPoolMembersPerPool (r:0 w:1)
	// Storage: NominationPools MinCreateBond (r:0 w:1)
	// Storage: NominationPools MaxPools (r:0 w:1)
	fn set_configs() -> Weight {
		// Minimum execution time: 6_051 nanoseconds.
		Weight::from_ref_time(6_313_000)
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: NominationPools BondedPools (r:1 w:1)
	fn update_roles() -> Weight {
		// Minimum execution time: 25_483 nanoseconds.
		Weight::from_ref_time(26_560_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: NominationPools BondedPools (r:1 w:0)
	// Storage: Staking Bonded (r:1 w:0)
	// Storage: Staking Ledger (r:1 w:0)
	// Storage: Staking Validators (r:1 w:0)
	// Storage: Staking Nominators (r:1 w:1)
	// Storage: Staking CounterForNominators (r:1 w:1)
	// Storage: VoterList ListNodes (r:1 w:1)
	// Storage: VoterList ListBags (r:1 w:1)
	// Storage: VoterList CounterForListNodes (r:1 w:1)
	fn chill() -> Weight {
		// Minimum execution time: 67_500 nanoseconds.
		Weight::from_ref_time(70_052_000)
			.saturating_add(T::DbWeight::get().reads(9))
			.saturating_add(T::DbWeight::get().writes(5))
	}
}
