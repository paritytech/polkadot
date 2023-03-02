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
	/// Storage: NominationPools MinJoinBond (r:1 w:0)
	/// Proof: NominationPools MinJoinBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:2 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembersPerPool (r:1 w:0)
	/// Proof: NominationPools MaxPoolMembersPerPool (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembers (r:1 w:0)
	/// Proof: NominationPools MaxPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	/// Proof: NominationPools CounterForPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:3 w:3)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:2 w:2)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	fn join() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `3468`
		//  Estimated: `38468`
		// Minimum execution time: 153_784 nanoseconds.
		Weight::from_ref_time(155_121_000)
			.saturating_add(Weight::from_proof_size(38468))
			.saturating_add(T::DbWeight::get().reads(17))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:3 w:2)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:3 w:3)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:2 w:2)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	fn bond_extra_transfer() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `3510`
		//  Estimated: `39063`
		// Minimum execution time: 151_609 nanoseconds.
		Weight::from_ref_time(155_385_000)
			.saturating_add(Weight::from_proof_size(39063))
			.saturating_add(T::DbWeight::get().reads(14))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	/// Storage: NominationPools ClaimPermissions (r:1 w:0)
	/// Proof: NominationPools ClaimPermissions (max_values: None, max_size: Some(41), added: 2516, mode: MaxEncodedLen)
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:3 w:3)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:2 w:2)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:2 w:2)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	fn bond_extra_other() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `3393`
		//  Estimated: `38950`
		// Minimum execution time: 165_036 nanoseconds.
		Weight::from_ref_time(166_264_000)
			.saturating_add(Weight::from_proof_size(38950))
			.saturating_add(T::DbWeight::get().reads(14))
			.saturating_add(T::DbWeight::get().writes(12))
	}
	/// Storage: NominationPools ClaimPermissions (r:1 w:0)
	/// Proof: NominationPools ClaimPermissions (max_values: None, max_size: Some(41), added: 2516, mode: MaxEncodedLen)
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	fn claim_payout() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1220`
		//  Estimated: `13485`
		// Minimum execution time: 61_052 nanoseconds.
		Weight::from_ref_time(61_773_000)
			.saturating_add(Weight::from_proof_size(13485))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: System Account (r:2 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking Nominators (r:1 w:0)
	/// Proof: Staking Nominators (max_values: None, max_size: Some(558), added: 3033, mode: MaxEncodedLen)
	/// Storage: Staking MinNominatorBond (r:1 w:0)
	/// Proof: Staking MinNominatorBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:3 w:3)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:2 w:2)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	/// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	/// Proof: NominationPools SubPoolsStorage (max_values: None, max_size: Some(1197), added: 3672, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForSubPoolsStorage (r:1 w:1)
	/// Proof: NominationPools CounterForSubPoolsStorage (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn unbond() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `3748`
		//  Estimated: `44674`
		// Minimum execution time: 155_226 nanoseconds.
		Weight::from_ref_time(155_893_000)
			.saturating_add(Weight::from_proof_size(44674))
			.saturating_add(T::DbWeight::get().reads(18))
			.saturating_add(T::DbWeight::get().writes(13))
	}
	/// Storage: NominationPools BondedPools (r:1 w:0)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 100]`.
	fn pool_withdraw_unbonded(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1672`
		//  Estimated: `13025`
		// Minimum execution time: 53_472 nanoseconds.
		Weight::from_ref_time(55_368_257)
			.saturating_add(Weight::from_proof_size(13025))
			// Standard Error: 904
			.saturating_add(Weight::from_ref_time(4_928).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	/// Proof: NominationPools SubPoolsStorage (max_values: None, max_size: Some(1197), added: 3672, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	/// Proof: NominationPools CounterForPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools ClaimPermissions (r:0 w:1)
	/// Proof: NominationPools ClaimPermissions (max_values: None, max_size: Some(41), added: 2516, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_update(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `2196`
		//  Estimated: `22991`
		// Minimum execution time: 103_698 nanoseconds.
		Weight::from_ref_time(105_075_914)
			.saturating_add(Weight::from_proof_size(22991))
			// Standard Error: 1_503
			.saturating_add(Weight::from_ref_time(19_668).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(9))
			.saturating_add(T::DbWeight::get().writes(8))
	}
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools SubPoolsStorage (r:1 w:1)
	/// Proof: NominationPools SubPoolsStorage (max_values: None, max_size: Some(1197), added: 3672, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:1)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Staking SlashingSpans (r:1 w:0)
	/// Proof Skipped: Staking SlashingSpans (max_values: None, max_size: None, mode: Measured)
	/// Storage: Staking Validators (r:1 w:0)
	/// Proof: Staking Validators (max_values: None, max_size: Some(45), added: 2520, mode: MaxEncodedLen)
	/// Storage: Staking Nominators (r:1 w:0)
	/// Proof: Staking Nominators (max_values: None, max_size: Some(558), added: 3033, mode: MaxEncodedLen)
	/// Storage: System Account (r:2 w:2)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	/// Proof: NominationPools CounterForPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools ReversePoolIdLookup (r:1 w:1)
	/// Proof: NominationPools ReversePoolIdLookup (max_values: None, max_size: Some(44), added: 2519, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForReversePoolIdLookup (r:1 w:1)
	/// Proof: NominationPools CounterForReversePoolIdLookup (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForRewardPools (r:1 w:1)
	/// Proof: NominationPools CounterForRewardPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForSubPoolsStorage (r:1 w:1)
	/// Proof: NominationPools CounterForSubPoolsStorage (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools Metadata (r:1 w:1)
	/// Proof: NominationPools Metadata (max_values: None, max_size: Some(270), added: 2745, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForBondedPools (r:1 w:1)
	/// Proof: NominationPools CounterForBondedPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking Payee (r:0 w:1)
	/// Proof: Staking Payee (max_values: None, max_size: Some(73), added: 2548, mode: MaxEncodedLen)
	/// Storage: NominationPools ClaimPermissions (r:0 w:1)
	/// Proof: NominationPools ClaimPermissions (max_values: None, max_size: Some(41), added: 2516, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 100]`.
	fn withdraw_unbonded_kill(_s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `2584`
		//  Estimated: `46001`
		// Minimum execution time: 163_337 nanoseconds.
		Weight::from_ref_time(167_095_416)
			.saturating_add(Weight::from_proof_size(46001))
			.saturating_add(T::DbWeight::get().reads(20))
			.saturating_add(T::DbWeight::get().writes(18))
	}
	/// Storage: NominationPools LastPoolId (r:1 w:1)
	/// Proof: NominationPools LastPoolId (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking MinNominatorBond (r:1 w:0)
	/// Proof: Staking MinNominatorBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools MinCreateBond (r:1 w:0)
	/// Proof: NominationPools MinCreateBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools MinJoinBond (r:1 w:0)
	/// Proof: NominationPools MinJoinBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPools (r:1 w:0)
	/// Proof: NominationPools MaxPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForBondedPools (r:1 w:1)
	/// Proof: NominationPools CounterForBondedPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools PoolMembers (r:1 w:1)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembersPerPool (r:1 w:0)
	/// Proof: NominationPools MaxPoolMembersPerPool (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembers (r:1 w:0)
	/// Proof: NominationPools MaxPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForPoolMembers (r:1 w:1)
	/// Proof: NominationPools CounterForPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: System Account (r:2 w:2)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:1)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:1)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Balances Locks (r:1 w:1)
	/// Proof: Balances Locks (max_values: None, max_size: Some(1299), added: 3774, mode: MaxEncodedLen)
	/// Storage: NominationPools RewardPools (r:1 w:1)
	/// Proof: NominationPools RewardPools (max_values: None, max_size: Some(60), added: 2535, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForRewardPools (r:1 w:1)
	/// Proof: NominationPools CounterForRewardPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools ReversePoolIdLookup (r:1 w:1)
	/// Proof: NominationPools ReversePoolIdLookup (max_values: None, max_size: Some(44), added: 2519, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForReversePoolIdLookup (r:1 w:1)
	/// Proof: NominationPools CounterForReversePoolIdLookup (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Payee (r:0 w:1)
	/// Proof: Staking Payee (max_values: None, max_size: Some(73), added: 2548, mode: MaxEncodedLen)
	fn create() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1220`
		//  Estimated: `32002`
		// Minimum execution time: 149_656 nanoseconds.
		Weight::from_ref_time(150_555_000)
			.saturating_add(Weight::from_proof_size(32002))
			.saturating_add(T::DbWeight::get().reads(21))
			.saturating_add(T::DbWeight::get().writes(15))
	}
	/// Storage: NominationPools BondedPools (r:1 w:0)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:0)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Staking MinNominatorBond (r:1 w:0)
	/// Proof: Staking MinNominatorBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: Staking Nominators (r:1 w:1)
	/// Proof: Staking Nominators (max_values: None, max_size: Some(558), added: 3033, mode: MaxEncodedLen)
	/// Storage: Staking MaxNominatorsCount (r:1 w:0)
	/// Proof: Staking MaxNominatorsCount (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking Validators (r:17 w:0)
	/// Proof: Staking Validators (max_values: None, max_size: Some(45), added: 2520, mode: MaxEncodedLen)
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:1 w:1)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:1 w:1)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	/// Storage: VoterList CounterForListNodes (r:1 w:1)
	/// Proof: VoterList CounterForListNodes (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking CounterForNominators (r:1 w:1)
	/// Proof: Staking CounterForNominators (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// The range of component `n` is `[1, 16]`.
	fn nominate(n: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1805`
		//  Estimated: `21998 + n * (2520 ±0)`
		// Minimum execution time: 66_157 nanoseconds.
		Weight::from_ref_time(66_972_809)
			.saturating_add(Weight::from_proof_size(21998))
			// Standard Error: 9_409
			.saturating_add(Weight::from_ref_time(1_327_035).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(12))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(5))
			.saturating_add(Weight::from_parts(0, 2520).saturating_mul(n.into()))
	}
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:0)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	fn set_state() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1393`
		//  Estimated: `8752`
		// Minimum execution time: 34_743 nanoseconds.
		Weight::from_ref_time(35_038_000)
			.saturating_add(Weight::from_proof_size(8752))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: NominationPools BondedPools (r:1 w:0)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: NominationPools Metadata (r:1 w:1)
	/// Proof: NominationPools Metadata (max_values: None, max_size: Some(270), added: 2745, mode: MaxEncodedLen)
	/// Storage: NominationPools CounterForMetadata (r:1 w:1)
	/// Proof: NominationPools CounterForMetadata (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// The range of component `n` is `[1, 256]`.
	fn set_metadata(n: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `525`
		//  Estimated: `5883`
		// Minimum execution time: 14_305 nanoseconds.
		Weight::from_ref_time(14_805_073)
			.saturating_add(Weight::from_proof_size(5883))
			// Standard Error: 125
			.saturating_add(Weight::from_ref_time(1_944).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: NominationPools MinJoinBond (r:0 w:1)
	/// Proof: NominationPools MinJoinBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembers (r:0 w:1)
	/// Proof: NominationPools MaxPoolMembers (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPoolMembersPerPool (r:0 w:1)
	/// Proof: NominationPools MaxPoolMembersPerPool (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: NominationPools MinCreateBond (r:0 w:1)
	/// Proof: NominationPools MinCreateBond (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: NominationPools MaxPools (r:0 w:1)
	/// Proof: NominationPools MaxPools (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn set_configs() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 5_054 nanoseconds.
		Weight::from_ref_time(5_333_000)
			.saturating_add(Weight::from_proof_size(0))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	/// Storage: NominationPools BondedPools (r:1 w:1)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	fn update_roles() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `525`
		//  Estimated: `2639`
		// Minimum execution time: 20_025 nanoseconds.
		Weight::from_ref_time(20_643_000)
			.saturating_add(Weight::from_proof_size(2639))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: NominationPools BondedPools (r:1 w:0)
	/// Proof: NominationPools BondedPools (max_values: None, max_size: Some(164), added: 2639, mode: MaxEncodedLen)
	/// Storage: Staking Bonded (r:1 w:0)
	/// Proof: Staking Bonded (max_values: None, max_size: Some(72), added: 2547, mode: MaxEncodedLen)
	/// Storage: Staking Ledger (r:1 w:0)
	/// Proof: Staking Ledger (max_values: None, max_size: Some(1091), added: 3566, mode: MaxEncodedLen)
	/// Storage: Staking Validators (r:1 w:0)
	/// Proof: Staking Validators (max_values: None, max_size: Some(45), added: 2520, mode: MaxEncodedLen)
	/// Storage: Staking Nominators (r:1 w:1)
	/// Proof: Staking Nominators (max_values: None, max_size: Some(558), added: 3033, mode: MaxEncodedLen)
	/// Storage: Staking CounterForNominators (r:1 w:1)
	/// Proof: Staking CounterForNominators (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: VoterList ListNodes (r:1 w:1)
	/// Proof: VoterList ListNodes (max_values: None, max_size: Some(154), added: 2629, mode: MaxEncodedLen)
	/// Storage: VoterList ListBags (r:1 w:1)
	/// Proof: VoterList ListBags (max_values: None, max_size: Some(82), added: 2557, mode: MaxEncodedLen)
	/// Storage: VoterList CounterForListNodes (r:1 w:1)
	/// Proof: VoterList CounterForListNodes (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	fn chill() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `2032`
		//  Estimated: `20489`
		// Minimum execution time: 65_418 nanoseconds.
		Weight::from_ref_time(65_853_000)
			.saturating_add(Weight::from_proof_size(20489))
			.saturating_add(T::DbWeight::get().reads(9))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	/// Storage: NominationPools PoolMembers (r:1 w:0)
	/// Proof: NominationPools PoolMembers (max_values: None, max_size: Some(717), added: 3192, mode: MaxEncodedLen)
	/// Storage: NominationPools ClaimPermissions (r:1 w:1)
	/// Proof: NominationPools ClaimPermissions (max_values: None, max_size: Some(41), added: 2516, mode: MaxEncodedLen)
	fn set_claim_permission() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `508`
		//  Estimated: `5708`
		// Minimum execution time: 14_918 nanoseconds.
		Weight::from_ref_time(15_304_000)
			.saturating_add(Weight::from_proof_size(5708))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
}
