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

//! Autogenerated weights for `pallet_election_provider_multi_phase`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-04-28, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_election_provider_multi_phase
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `pallet_election_provider_multi_phase`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_election_provider_multi_phase::WeightInfo for WeightInfo<T> {
	/// Storage: Staking CurrentEra (r:1 w:0)
	/// Proof: Staking CurrentEra (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking CurrentPlannedSession (r:1 w:0)
	/// Proof: Staking CurrentPlannedSession (max_values: Some(1), max_size: Some(4), added: 499, mode: MaxEncodedLen)
	/// Storage: Staking ErasStartSessionIndex (r:1 w:0)
	/// Proof: Staking ErasStartSessionIndex (max_values: None, max_size: Some(16), added: 2491, mode: MaxEncodedLen)
	/// Storage: Babe EpochIndex (r:1 w:0)
	/// Proof: Babe EpochIndex (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Babe GenesisSlot (r:1 w:0)
	/// Proof: Babe GenesisSlot (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Babe CurrentSlot (r:1 w:0)
	/// Proof: Babe CurrentSlot (max_values: Some(1), max_size: Some(8), added: 503, mode: MaxEncodedLen)
	/// Storage: Staking ForceEra (r:1 w:0)
	/// Proof: Staking ForceEra (max_values: Some(1), max_size: Some(1), added: 496, mode: MaxEncodedLen)
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	fn on_initialize_nothing() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `993`
		//  Estimated: `3481`
		// Minimum execution time: 18_521_000 picoseconds.
		Weight::from_parts(19_112_000, 0)
			.saturating_add(Weight::from_parts(0, 3481))
			.saturating_add(T::DbWeight::get().reads(8))
	}
	/// Storage: ElectionProviderMultiPhase Round (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Round (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	fn on_initialize_open_signed() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `114`
		//  Estimated: `1599`
		// Minimum execution time: 13_405_000 picoseconds.
		Weight::from_parts(13_834_000, 0)
			.saturating_add(Weight::from_parts(0, 1599))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: ElectionProviderMultiPhase Round (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Round (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	fn on_initialize_open_unsigned() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `114`
		//  Estimated: `1599`
		// Minimum execution time: 14_598_000 picoseconds.
		Weight::from_parts(15_035_000, 0)
			.saturating_add(Weight::from_parts(0, 1599))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: ElectionProviderMultiPhase QueuedSolution (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase QueuedSolution (max_values: Some(1), max_size: None, mode: Measured)
	fn finalize_signed_phase_accept_solution() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `174`
		//  Estimated: `3593`
		// Minimum execution time: 30_510_000 picoseconds.
		Weight::from_parts(31_162_000, 0)
			.saturating_add(Weight::from_parts(0, 3593))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	fn finalize_signed_phase_reject_solution() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `174`
		//  Estimated: `3593`
		// Minimum execution time: 21_632_000 picoseconds.
		Weight::from_parts(22_098_000, 0)
			.saturating_add(Weight::from_parts(0, 3593))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: ElectionProviderMultiPhase SnapshotMetadata (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SnapshotMetadata (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase DesiredTargets (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase DesiredTargets (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Snapshot (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase Snapshot (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `v` is `[1000, 2000]`.
	/// The range of component `t` is `[500, 1000]`.
	fn create_snapshot_internal(v: u32, _t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 455_544_000 picoseconds.
		Weight::from_parts(470_334_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
			// Standard Error: 2_558
			.saturating_add(Weight::from_parts(270_771, 0).saturating_mul(v.into()))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: ElectionProviderMultiPhase SignedSubmissionIndices (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionIndices (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SignedSubmissionNextIndex (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionNextIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SnapshotMetadata (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SnapshotMetadata (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SignedSubmissionsMap (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionsMap (max_values: None, max_size: None, mode: Measured)
	/// Storage: System BlockWeight (r:1 w:1)
	/// Proof: System BlockWeight (max_values: Some(1), max_size: Some(48), added: 543, mode: MaxEncodedLen)
	/// Storage: ElectionProviderMultiPhase QueuedSolution (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase QueuedSolution (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Round (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase Round (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase DesiredTargets (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase DesiredTargets (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Snapshot (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase Snapshot (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `a` is `[500, 800]`.
	/// The range of component `d` is `[200, 400]`.
	fn elect_queued(a: u32, d: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `338 + a * (768 ±0) + d * (48 ±0)`
		//  Estimated: `3890 + a * (768 ±0) + d * (49 ±0)`
		// Minimum execution time: 285_740_000 picoseconds.
		Weight::from_parts(63_863_280, 0)
			.saturating_add(Weight::from_parts(0, 3890))
			// Standard Error: 3_212
			.saturating_add(Weight::from_parts(353_729, 0).saturating_mul(a.into()))
			// Standard Error: 4_815
			.saturating_add(Weight::from_parts(155_831, 0).saturating_mul(d.into()))
			.saturating_add(T::DbWeight::get().reads(8))
			.saturating_add(T::DbWeight::get().writes(9))
			.saturating_add(Weight::from_parts(0, 768).saturating_mul(a.into()))
			.saturating_add(Weight::from_parts(0, 49).saturating_mul(d.into()))
	}
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SnapshotMetadata (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase SnapshotMetadata (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: TransactionPayment NextFeeMultiplier (r:1 w:0)
	/// Proof: TransactionPayment NextFeeMultiplier (max_values: Some(1), max_size: Some(16), added: 511, mode: MaxEncodedLen)
	/// Storage: ElectionProviderMultiPhase SignedSubmissionIndices (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionIndices (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SignedSubmissionNextIndex (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionNextIndex (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SignedSubmissionsMap (r:0 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase SignedSubmissionsMap (max_values: None, max_size: None, mode: Measured)
	fn submit() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1204`
		//  Estimated: `2689`
		// Minimum execution time: 47_143_000 picoseconds.
		Weight::from_parts(47_590_000, 0)
			.saturating_add(Weight::from_parts(0, 2689))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: ElectionProviderMultiPhase CurrentPhase (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase CurrentPhase (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Round (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Round (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase DesiredTargets (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase DesiredTargets (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase QueuedSolution (r:1 w:1)
	/// Proof Skipped: ElectionProviderMultiPhase QueuedSolution (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase SnapshotMetadata (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase SnapshotMetadata (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Snapshot (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Snapshot (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase MinimumUntrustedScore (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase MinimumUntrustedScore (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `v` is `[1000, 2000]`.
	/// The range of component `t` is `[500, 1000]`.
	/// The range of component `a` is `[500, 800]`.
	/// The range of component `d` is `[200, 400]`.
	fn submit_unsigned(v: u32, t: u32, a: u32, _d: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `219 + t * (32 ±0) + v * (553 ±0)`
		//  Estimated: `1704 + t * (32 ±0) + v * (553 ±0)`
		// Minimum execution time: 5_089_555_000 picoseconds.
		Weight::from_parts(5_107_135_000, 0)
			.saturating_add(Weight::from_parts(0, 1704))
			// Standard Error: 16_098
			.saturating_add(Weight::from_parts(61_662, 0).saturating_mul(v.into()))
			// Standard Error: 47_704
			.saturating_add(Weight::from_parts(4_710_124, 0).saturating_mul(a.into()))
			.saturating_add(T::DbWeight::get().reads(7))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(Weight::from_parts(0, 32).saturating_mul(t.into()))
			.saturating_add(Weight::from_parts(0, 553).saturating_mul(v.into()))
	}
	/// Storage: ElectionProviderMultiPhase DesiredTargets (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase DesiredTargets (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Snapshot (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Snapshot (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase Round (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase Round (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: ElectionProviderMultiPhase MinimumUntrustedScore (r:1 w:0)
	/// Proof Skipped: ElectionProviderMultiPhase MinimumUntrustedScore (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `v` is `[1000, 2000]`.
	/// The range of component `t` is `[500, 1000]`.
	/// The range of component `a` is `[500, 800]`.
	/// The range of component `d` is `[200, 400]`.
	fn feasibility_check(v: u32, t: u32, a: u32, _d: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `194 + t * (32 ±0) + v * (553 ±0)`
		//  Estimated: `1679 + t * (32 ±0) + v * (553 ±0)`
		// Minimum execution time: 4_305_785_000 picoseconds.
		Weight::from_parts(4_330_126_000, 0)
			.saturating_add(Weight::from_parts(0, 1679))
			// Standard Error: 13_362
			.saturating_add(Weight::from_parts(175_613, 0).saturating_mul(v.into()))
			// Standard Error: 39_597
			.saturating_add(Weight::from_parts(3_618_103, 0).saturating_mul(a.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(Weight::from_parts(0, 32).saturating_mul(t.into()))
			.saturating_add(Weight::from_parts(0, 553).saturating_mul(v.into()))
	}
}
