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
//! Autogenerated weights for `runtime_parachains::paras`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-11-16, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm4`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=runtime_parachains::paras
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/runtime_parachains_paras.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_parachains::paras`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::paras::WeightInfo for WeightInfo<T> {
	// Storage: Paras CurrentCodeHash (r:1 w:1)
	// Storage: Paras CodeByHashRefs (r:1 w:1)
	// Storage: Paras PastCodeMeta (r:1 w:1)
	// Storage: Paras PastCodePruning (r:1 w:1)
	// Storage: Paras PastCodeHash (r:0 w:1)
	// Storage: Paras CodeByHash (r:0 w:1)
	/// The range of component `c` is `[1, 3145728]`.
	fn force_set_current_code(c: u32, ) -> Weight {
		// Minimum execution time: 38_052 nanoseconds.
		Weight::from_ref_time(38_343_000 as u64)
			// Standard Error: 1
			.saturating_add(Weight::from_ref_time(2_470 as u64).saturating_mul(c as u64))
			.saturating_add(T::DbWeight::get().reads(4 as u64))
			.saturating_add(T::DbWeight::get().writes(6 as u64))
	}
	// Storage: Paras Heads (r:0 w:1)
	/// The range of component `s` is `[1, 1048576]`.
	fn force_set_current_head(s: u32, ) -> Weight {
		// Minimum execution time: 12_951 nanoseconds.
		Weight::from_ref_time(13_054_000 as u64)
			// Standard Error: 2
			.saturating_add(Weight::from_ref_time(1_071 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: Paras FutureCodeHash (r:1 w:1)
	// Storage: Paras CurrentCodeHash (r:1 w:0)
	// Storage: Paras UpgradeCooldowns (r:1 w:1)
	// Storage: Paras PvfActiveVoteMap (r:1 w:0)
	// Storage: Paras CodeByHash (r:1 w:1)
	// Storage: Paras UpcomingUpgrades (r:1 w:1)
	// Storage: System Digest (r:1 w:1)
	// Storage: Paras CodeByHashRefs (r:1 w:1)
	// Storage: Paras FutureCodeUpgrades (r:0 w:1)
	// Storage: Paras UpgradeRestrictionSignal (r:0 w:1)
	/// The range of component `c` is `[1, 3145728]`.
	fn force_schedule_code_upgrade(c: u32, ) -> Weight {
		// Minimum execution time: 62_248 nanoseconds.
		Weight::from_ref_time(62_476_000 as u64)
			// Standard Error: 1
			.saturating_add(Weight::from_ref_time(2_494 as u64).saturating_mul(c as u64))
			.saturating_add(T::DbWeight::get().reads(9 as u64))
			.saturating_add(T::DbWeight::get().writes(8 as u64))
	}
	// Storage: Paras FutureCodeUpgrades (r:1 w:0)
	// Storage: Paras Heads (r:0 w:1)
	// Storage: Paras UpgradeGoAheadSignal (r:0 w:1)
	/// The range of component `s` is `[1, 1048576]`.
	fn force_note_new_head(s: u32, ) -> Weight {
		// Minimum execution time: 18_755 nanoseconds.
		Weight::from_ref_time(19_028_000 as u64)
			// Standard Error: 2
			.saturating_add(Weight::from_ref_time(1_069 as u64).saturating_mul(s as u64))
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(2 as u64))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras ActionsQueue (r:1 w:1)
	fn force_queue_action() -> Weight {
		// Minimum execution time: 24_134 nanoseconds.
		Weight::from_ref_time(24_730_000 as u64)
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Paras PvfActiveVoteMap (r:1 w:0)
	// Storage: Paras CodeByHash (r:1 w:1)
	/// The range of component `c` is `[1, 3145728]`.
	fn add_trusted_validation_code(c: u32, ) -> Weight {
		// Minimum execution time: 8_618 nanoseconds.
		Weight::from_ref_time(8_808_000 as u64)
			// Standard Error: 1
			.saturating_add(Weight::from_ref_time(2_473 as u64).saturating_mul(c as u64))
			.saturating_add(T::DbWeight::get().reads(2 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Paras CodeByHashRefs (r:1 w:0)
	// Storage: Paras CodeByHash (r:0 w:1)
	fn poke_unused_validation_code() -> Weight {
		// Minimum execution time: 6_839 nanoseconds.
		Weight::from_ref_time(6_966_000 as u64)
			.saturating_add(T::DbWeight::get().reads(1 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	fn include_pvf_check_statement() -> Weight {
		// Minimum execution time: 92_622 nanoseconds.
		Weight::from_ref_time(94_867_000 as u64)
			.saturating_add(T::DbWeight::get().reads(4 as u64))
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras UpcomingUpgrades (r:1 w:1)
	// Storage: System Digest (r:1 w:1)
	// Storage: Paras FutureCodeUpgrades (r:0 w:100)
	fn include_pvf_check_statement_finalize_upgrade_accept() -> Weight {
		// Minimum execution time: 637_636 nanoseconds.
		Weight::from_ref_time(647_421_000 as u64)
			.saturating_add(T::DbWeight::get().reads(7 as u64))
			.saturating_add(T::DbWeight::get().writes(104 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras CodeByHashRefs (r:1 w:1)
	// Storage: Paras CodeByHash (r:0 w:1)
	// Storage: Paras UpgradeGoAheadSignal (r:0 w:100)
	// Storage: Paras FutureCodeHash (r:0 w:100)
	fn include_pvf_check_statement_finalize_upgrade_reject() -> Weight {
		// Minimum execution time: 593_660 nanoseconds.
		Weight::from_ref_time(601_930_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(204 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras ActionsQueue (r:1 w:1)
	fn include_pvf_check_statement_finalize_onboarding_accept() -> Weight {
		// Minimum execution time: 498_509 nanoseconds.
		Weight::from_ref_time(505_284_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}
	// Storage: Configuration ActiveConfig (r:1 w:0)
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras CodeByHashRefs (r:1 w:1)
	// Storage: Paras ParaLifecycles (r:0 w:100)
	// Storage: Paras CodeByHash (r:0 w:1)
	// Storage: Paras CurrentCodeHash (r:0 w:100)
	// Storage: Paras UpcomingParasGenesis (r:0 w:100)
	fn include_pvf_check_statement_finalize_onboarding_reject() -> Weight {
		// Minimum execution time: 669_947 nanoseconds.
		Weight::from_ref_time(676_418_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(304 as u64))
	}
}
