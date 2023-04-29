// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Autogenerated weights for `runtime_parachains::paras_inherent`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-04-26, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm3`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// target/production/polkadot
// benchmark
// pallet
// --steps=50
// --repeat=20
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --json-file=/var/lib/gitlab-runner/builds/zyw4fam_/0/parity/mirrors/polkadot/.git/.artifacts/bench.json
// --pallet=runtime_parachains::paras_inherent
// --chain=westend-dev
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `runtime_parachains::paras_inherent`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::paras_inherent::WeightInfo for WeightInfo<T> {
	/// Storage: ParaInherent Included (r:1 w:1)
	/// Proof Skipped: ParaInherent Included (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: System ParentHash (r:1 w:0)
	/// Proof: System ParentHash (max_values: Some(1), max_size: Some(32), added: 527, mode: MaxEncodedLen)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Babe AuthorVrfRandomness (r:1 w:0)
	/// Proof: Babe AuthorVrfRandomness (max_values: Some(1), max_size: Some(33), added: 528, mode: MaxEncodedLen)
	/// Storage: ParaSessionInfo Sessions (r:1 w:0)
	/// Proof Skipped: ParaSessionInfo Sessions (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasDisputes Disputes (r:1 w:1)
	/// Proof Skipped: ParasDisputes Disputes (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasDisputes BackersOnDisputes (r:1 w:1)
	/// Proof Skipped: ParasDisputes BackersOnDisputes (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaScheduler AvailabilityCores (r:1 w:1)
	/// Proof Skipped: ParaScheduler AvailabilityCores (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Included (r:1 w:1)
	/// Proof Skipped: ParasDisputes Included (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaSessionInfo AccountKeys (r:1 w:0)
	/// Proof Skipped: ParaSessionInfo AccountKeys (max_values: None, max_size: None, mode: Measured)
	/// Storage: Session Validators (r:1 w:0)
	/// Proof Skipped: Session Validators (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Staking ActiveEra (r:1 w:0)
	/// Proof: Staking ActiveEra (max_values: Some(1), max_size: Some(13), added: 508, mode: MaxEncodedLen)
	/// Storage: Staking ErasRewardPoints (r:1 w:1)
	/// Proof Skipped: Staking ErasRewardPoints (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasDisputes Frozen (r:1 w:0)
	/// Proof Skipped: ParasDisputes Frozen (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailability (r:2 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailability (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	/// Proof Skipped: ParasShared ActiveValidatorKeys (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Paras Parachains (r:1 w:0)
	/// Proof Skipped: Paras Parachains (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailabilityCommitments (r:1 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailabilityCommitments (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	/// Proof Skipped: Dmp DownwardMessageQueues (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DeliveryFeeFactor (r:1 w:1)
	/// Proof Skipped: Dmp DeliveryFeeFactor (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpChannelDigests (r:1 w:1)
	/// Proof Skipped: Hrmp HrmpChannelDigests (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras FutureCodeUpgrades (r:1 w:0)
	/// Proof Skipped: Paras FutureCodeUpgrades (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaInherent OnChainVotes (r:1 w:1)
	/// Proof Skipped: ParaInherent OnChainVotes (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler SessionStartBlock (r:1 w:0)
	/// Proof Skipped: ParaScheduler SessionStartBlock (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ParathreadQueue (r:1 w:1)
	/// Proof Skipped: ParaScheduler ParathreadQueue (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler Scheduled (r:1 w:1)
	/// Proof Skipped: ParaScheduler Scheduled (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ValidatorGroups (r:1 w:0)
	/// Proof Skipped: ParaScheduler ValidatorGroups (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Ump NeedsDispatch (r:1 w:1)
	/// Proof Skipped: Ump NeedsDispatch (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Ump NextDispatchRoundStartWith (r:1 w:1)
	/// Proof Skipped: Ump NextDispatchRoundStartWith (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpWatermarks (r:0 w:1)
	/// Proof Skipped: Hrmp HrmpWatermarks (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras Heads (r:0 w:1)
	/// Proof Skipped: Paras Heads (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras UpgradeGoAheadSignal (r:0 w:1)
	/// Proof Skipped: Paras UpgradeGoAheadSignal (max_values: None, max_size: None, mode: Measured)
	/// The range of component `v` is `[10, 200]`.
	fn enter_variable_disputes(v: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `50561`
		//  Estimated: `56501 + v * (23 ±0)`
		// Minimum execution time: 805_405_000 picoseconds.
		Weight::from_parts(342_195_461, 0)
			.saturating_add(Weight::from_parts(0, 56501))
			// Standard Error: 27_524
			.saturating_add(Weight::from_parts(48_351_060, 0).saturating_mul(v.into()))
			.saturating_add(T::DbWeight::get().reads(29))
			.saturating_add(T::DbWeight::get().writes(17))
			.saturating_add(Weight::from_parts(0, 23).saturating_mul(v.into()))
	}
	/// Storage: ParaInherent Included (r:1 w:1)
	/// Proof Skipped: ParaInherent Included (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: System ParentHash (r:1 w:0)
	/// Proof: System ParentHash (max_values: Some(1), max_size: Some(32), added: 527, mode: MaxEncodedLen)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Babe AuthorVrfRandomness (r:1 w:0)
	/// Proof: Babe AuthorVrfRandomness (max_values: Some(1), max_size: Some(33), added: 528, mode: MaxEncodedLen)
	/// Storage: ParaScheduler AvailabilityCores (r:1 w:1)
	/// Proof Skipped: ParaScheduler AvailabilityCores (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Frozen (r:1 w:0)
	/// Proof Skipped: ParasDisputes Frozen (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	/// Proof Skipped: ParasShared ActiveValidatorKeys (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Paras Parachains (r:1 w:0)
	/// Proof Skipped: Paras Parachains (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailability (r:2 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailability (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailabilityCommitments (r:1 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailabilityCommitments (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaSessionInfo AccountKeys (r:1 w:0)
	/// Proof Skipped: ParaSessionInfo AccountKeys (max_values: None, max_size: None, mode: Measured)
	/// Storage: Session Validators (r:1 w:0)
	/// Proof Skipped: Session Validators (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Staking ActiveEra (r:1 w:0)
	/// Proof: Staking ActiveEra (max_values: Some(1), max_size: Some(13), added: 508, mode: MaxEncodedLen)
	/// Storage: Staking ErasRewardPoints (r:1 w:1)
	/// Proof Skipped: Staking ErasRewardPoints (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	/// Proof Skipped: Dmp DownwardMessageQueues (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DeliveryFeeFactor (r:1 w:1)
	/// Proof Skipped: Dmp DeliveryFeeFactor (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpChannelDigests (r:1 w:1)
	/// Proof Skipped: Hrmp HrmpChannelDigests (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras FutureCodeUpgrades (r:1 w:0)
	/// Proof Skipped: Paras FutureCodeUpgrades (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaInherent OnChainVotes (r:1 w:1)
	/// Proof Skipped: ParaInherent OnChainVotes (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Disputes (r:1 w:0)
	/// Proof Skipped: ParasDisputes Disputes (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaScheduler SessionStartBlock (r:1 w:0)
	/// Proof Skipped: ParaScheduler SessionStartBlock (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ParathreadQueue (r:1 w:1)
	/// Proof Skipped: ParaScheduler ParathreadQueue (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler Scheduled (r:1 w:1)
	/// Proof Skipped: ParaScheduler Scheduled (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ValidatorGroups (r:1 w:0)
	/// Proof Skipped: ParaScheduler ValidatorGroups (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Ump NeedsDispatch (r:1 w:1)
	/// Proof Skipped: Ump NeedsDispatch (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Ump NextDispatchRoundStartWith (r:1 w:1)
	/// Proof Skipped: Ump NextDispatchRoundStartWith (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaInclusion AvailabilityBitfields (r:0 w:1)
	/// Proof Skipped: ParaInclusion AvailabilityBitfields (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParasDisputes Included (r:0 w:1)
	/// Proof Skipped: ParasDisputes Included (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpWatermarks (r:0 w:1)
	/// Proof Skipped: Hrmp HrmpWatermarks (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras Heads (r:0 w:1)
	/// Proof Skipped: Paras Heads (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras UpgradeGoAheadSignal (r:0 w:1)
	/// Proof Skipped: Paras UpgradeGoAheadSignal (max_values: None, max_size: None, mode: Measured)
	fn enter_bitfields() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `42395`
		//  Estimated: `48335`
		// Minimum execution time: 348_336_000 picoseconds.
		Weight::from_parts(358_001_000, 0)
			.saturating_add(Weight::from_parts(0, 48335))
			.saturating_add(T::DbWeight::get().reads(27))
			.saturating_add(T::DbWeight::get().writes(18))
	}
	/// Storage: ParaInherent Included (r:1 w:1)
	/// Proof Skipped: ParaInherent Included (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: System ParentHash (r:1 w:0)
	/// Proof: System ParentHash (max_values: Some(1), max_size: Some(32), added: 527, mode: MaxEncodedLen)
	/// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	/// Proof Skipped: ParasShared CurrentSessionIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Babe AuthorVrfRandomness (r:1 w:0)
	/// Proof: Babe AuthorVrfRandomness (max_values: Some(1), max_size: Some(33), added: 528, mode: MaxEncodedLen)
	/// Storage: ParaScheduler AvailabilityCores (r:1 w:1)
	/// Proof Skipped: ParaScheduler AvailabilityCores (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Frozen (r:1 w:0)
	/// Proof Skipped: ParasDisputes Frozen (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasShared ActiveValidatorKeys (r:1 w:0)
	/// Proof Skipped: ParasShared ActiveValidatorKeys (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Paras Parachains (r:1 w:0)
	/// Proof Skipped: Paras Parachains (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailability (r:2 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailability (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaInclusion PendingAvailabilityCommitments (r:1 w:1)
	/// Proof Skipped: ParaInclusion PendingAvailabilityCommitments (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaSessionInfo AccountKeys (r:1 w:0)
	/// Proof Skipped: ParaSessionInfo AccountKeys (max_values: None, max_size: None, mode: Measured)
	/// Storage: Session Validators (r:1 w:0)
	/// Proof Skipped: Session Validators (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Staking ActiveEra (r:1 w:0)
	/// Proof: Staking ActiveEra (max_values: Some(1), max_size: Some(13), added: 508, mode: MaxEncodedLen)
	/// Storage: Staking ErasRewardPoints (r:1 w:1)
	/// Proof Skipped: Staking ErasRewardPoints (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	/// Proof Skipped: Dmp DownwardMessageQueues (max_values: None, max_size: None, mode: Measured)
	/// Storage: Dmp DeliveryFeeFactor (r:1 w:1)
	/// Proof Skipped: Dmp DeliveryFeeFactor (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpChannelDigests (r:1 w:1)
	/// Proof Skipped: Hrmp HrmpChannelDigests (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras FutureCodeUpgrades (r:1 w:0)
	/// Proof Skipped: Paras FutureCodeUpgrades (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaInherent OnChainVotes (r:1 w:1)
	/// Proof Skipped: ParaInherent OnChainVotes (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Disputes (r:2 w:0)
	/// Proof Skipped: ParasDisputes Disputes (max_values: None, max_size: None, mode: Measured)
	/// Storage: ParaScheduler SessionStartBlock (r:1 w:0)
	/// Proof Skipped: ParaScheduler SessionStartBlock (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ParathreadQueue (r:1 w:1)
	/// Proof Skipped: ParaScheduler ParathreadQueue (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler Scheduled (r:1 w:1)
	/// Proof Skipped: ParaScheduler Scheduled (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParaScheduler ValidatorGroups (r:1 w:0)
	/// Proof Skipped: ParaScheduler ValidatorGroups (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Paras CurrentCodeHash (r:1 w:0)
	/// Proof Skipped: Paras CurrentCodeHash (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras FutureCodeHash (r:1 w:0)
	/// Proof Skipped: Paras FutureCodeHash (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras UpgradeRestrictionSignal (r:1 w:0)
	/// Proof Skipped: Paras UpgradeRestrictionSignal (max_values: None, max_size: None, mode: Measured)
	/// Storage: Ump RelayDispatchQueueSize (r:1 w:0)
	/// Proof Skipped: Ump RelayDispatchQueueSize (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpChannels (r:1 w:0)
	/// Proof Skipped: Hrmp HrmpChannels (max_values: None, max_size: None, mode: Measured)
	/// Storage: Ump NeedsDispatch (r:1 w:1)
	/// Proof Skipped: Ump NeedsDispatch (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Ump NextDispatchRoundStartWith (r:1 w:1)
	/// Proof Skipped: Ump NextDispatchRoundStartWith (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ParasDisputes Included (r:0 w:1)
	/// Proof Skipped: ParasDisputes Included (max_values: None, max_size: None, mode: Measured)
	/// Storage: Hrmp HrmpWatermarks (r:0 w:1)
	/// Proof Skipped: Hrmp HrmpWatermarks (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras Heads (r:0 w:1)
	/// Proof Skipped: Paras Heads (max_values: None, max_size: None, mode: Measured)
	/// Storage: Paras UpgradeGoAheadSignal (r:0 w:1)
	/// Proof Skipped: Paras UpgradeGoAheadSignal (max_values: None, max_size: None, mode: Measured)
	/// The range of component `v` is `[2, 5]`.
	/// The range of component `u` is `[0, 10]`.
	/// The range of component `h` is `[0, 10]`.
	/// The range of component `c` is `[0, 10]`.
	fn enter_backed_candidate(v: u32, u: u32, h: u32, c: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `42518`
		//  Estimated: `48408 + c * (1 ±0) + h * (3 ±0) + u * (1 ±0) + v * (2 ±0)`
		// Minimum execution time: 869_838_000 picoseconds.
		Weight::from_parts(780_250_196, 0)
			.saturating_add(Weight::from_parts(0, 48408))
			// Standard Error: 520_761
			.saturating_add(Weight::from_parts(57_984_625, 0).saturating_mul(v.into()))
			// Standard Error: 177_867
			.saturating_add(Weight::from_parts(201_843, 0).saturating_mul(h.into()))
			// Standard Error: 177_867
			.saturating_add(Weight::from_parts(176_126, 0).saturating_mul(c.into()))
			.saturating_add(T::DbWeight::get().reads(31))
			.saturating_add(T::DbWeight::get().writes(17))
			.saturating_add(Weight::from_parts(0, 1).saturating_mul(c.into()))
			.saturating_add(Weight::from_parts(0, 3).saturating_mul(h.into()))
			.saturating_add(Weight::from_parts(0, 1).saturating_mul(u.into()))
			.saturating_add(Weight::from_parts(0, 2).saturating_mul(v.into()))
	}
}
