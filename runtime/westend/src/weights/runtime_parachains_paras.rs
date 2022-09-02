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
// --pallet=runtime_parachains::paras
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/runtime_parachains_paras.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{RefTimeWeight, Weight}};
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
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(3_000 as RefTimeWeight).saturating_mul(c as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(4 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(6 as RefTimeWeight))
	}
	// Storage: Paras Heads (r:0 w:1)
	/// The range of component `s` is `[1, 1048576]`.
	fn force_set_current_head(s: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(1_000 as RefTimeWeight).saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
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
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(3_000 as RefTimeWeight).saturating_mul(c as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(8 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(8 as RefTimeWeight))
	}
	// Storage: Paras FutureCodeUpgrades (r:1 w:0)
	// Storage: Paras Heads (r:0 w:1)
	// Storage: Paras UpgradeGoAheadSignal (r:0 w:1)
	/// The range of component `s` is `[1, 1048576]`.
	fn force_note_new_head(s: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(1_000 as RefTimeWeight).saturating_mul(s as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras ActionsQueue (r:1 w:1)
	fn force_queue_action() -> Weight {
		Weight::from_ref_time(19_269_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Paras PvfActiveVoteMap (r:1 w:0)
	// Storage: Paras CodeByHash (r:1 w:1)
	/// The range of component `c` is `[1, 3145728]`.
	fn add_trusted_validation_code(c: u32, ) -> Weight {
		Weight::from_ref_time(0 as RefTimeWeight)
			// Standard Error: 0
			.saturating_add(Weight::from_ref_time(3_000 as RefTimeWeight).saturating_mul(c as RefTimeWeight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Paras CodeByHashRefs (r:1 w:0)
	// Storage: Paras CodeByHash (r:0 w:1)
	fn poke_unused_validation_code() -> Weight {
		Weight::from_ref_time(4_769_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	fn include_pvf_check_statement() -> Weight {
		Weight::from_ref_time(92_142_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras UpcomingUpgrades (r:1 w:1)
	// Storage: System Digest (r:1 w:1)
	// Storage: Paras FutureCodeUpgrades (r:0 w:100)
	fn include_pvf_check_statement_finalize_upgrade_accept() -> Weight {
		Weight::from_ref_time(680_774_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(6 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(104 as RefTimeWeight))
	}
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras CodeByHashRefs (r:1 w:1)
	// Storage: Paras CodeByHash (r:0 w:1)
	// Storage: Paras UpgradeGoAheadSignal (r:0 w:100)
	// Storage: Paras FutureCodeHash (r:0 w:100)
	fn include_pvf_check_statement_finalize_upgrade_reject() -> Weight {
		Weight::from_ref_time(630_172_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(5 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(204 as RefTimeWeight))
	}
	// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Paras PvfActiveVoteMap (r:1 w:1)
	// Storage: Paras PvfActiveVoteList (r:1 w:1)
	// Storage: Paras ActionsQueue (r:1 w:1)
	fn include_pvf_check_statement_finalize_onboarding_accept() -> Weight {
		Weight::from_ref_time(535_446_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(5 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(3 as RefTimeWeight))
	}
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
		Weight::from_ref_time(702_781_000 as RefTimeWeight)
			.saturating_add(T::DbWeight::get().reads(5 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(304 as RefTimeWeight))
	}
}
