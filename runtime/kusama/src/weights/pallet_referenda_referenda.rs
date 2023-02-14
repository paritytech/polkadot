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
//! Autogenerated weights for `pallet_referenda`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-01-23, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `runner-b3zmxxc-project-163-concurrent-0`, CPU: `Intel(R) Xeon(R) CPU @ 2.60GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_referenda
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_referenda`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_referenda::WeightInfo for WeightInfo<T> {
	// Storage: Referenda ReferendumCount (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	// Storage: Referenda ReferendumInfoFor (r:0 w:1)
	fn submit() -> Weight {
		// Minimum execution time: 38_444 nanoseconds.
		Weight::from_ref_time(40_157_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn place_decision_deposit_preparing() -> Weight {
		// Minimum execution time: 52_464 nanoseconds.
		Weight::from_ref_time(53_840_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:0)
	// Storage: Referenda TrackQueue (r:1 w:1)
	fn place_decision_deposit_queued() -> Weight {
		// Minimum execution time: 59_684 nanoseconds.
		Weight::from_ref_time(62_639_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:0)
	// Storage: Referenda TrackQueue (r:1 w:1)
	fn place_decision_deposit_not_queued() -> Weight {
		// Minimum execution time: 59_614 nanoseconds.
		Weight::from_ref_time(61_921_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn place_decision_deposit_passing() -> Weight {
		// Minimum execution time: 67_948 nanoseconds.
		Weight::from_ref_time(69_857_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	fn place_decision_deposit_failing() -> Weight {
		// Minimum execution time: 46_712 nanoseconds.
		Weight::from_ref_time(47_618_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	fn refund_decision_deposit() -> Weight {
		// Minimum execution time: 31_886 nanoseconds.
		Weight::from_ref_time(33_383_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	fn refund_submission_deposit() -> Weight {
		// Minimum execution time: 31_464 nanoseconds.
		Weight::from_ref_time(32_332_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn cancel() -> Weight {
		// Minimum execution time: 40_661 nanoseconds.
		Weight::from_ref_time(42_037_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn kill() -> Weight {
		// Minimum execution time: 87_937 nanoseconds.
		Weight::from_ref_time(90_171_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda TrackQueue (r:1 w:0)
	// Storage: Referenda DecidingCount (r:1 w:1)
	fn one_fewer_deciding_queue_empty() -> Weight {
		// Minimum execution time: 10_962 nanoseconds.
		Weight::from_ref_time(11_292_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn one_fewer_deciding_failing() -> Weight {
		// Minimum execution time: 123_968 nanoseconds.
		Weight::from_ref_time(132_776_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn one_fewer_deciding_passing() -> Weight {
		// Minimum execution time: 124_456 nanoseconds.
		Weight::from_ref_time(133_803_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_requeued_insertion() -> Weight {
		// Minimum execution time: 67_800 nanoseconds.
		Weight::from_ref_time(72_433_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_requeued_slide() -> Weight {
		// Minimum execution time: 67_119 nanoseconds.
		Weight::from_ref_time(72_291_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:0)
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_queued() -> Weight {
		// Minimum execution time: 69_519 nanoseconds.
		Weight::from_ref_time(75_088_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:0)
	// Storage: Referenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_not_queued() -> Weight {
		// Minimum execution time: 68_612 nanoseconds.
		Weight::from_ref_time(75_595_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_no_deposit() -> Weight {
		// Minimum execution time: 29_767 nanoseconds.
		Weight::from_ref_time(30_670_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_preparing() -> Weight {
		// Minimum execution time: 30_082 nanoseconds.
		Weight::from_ref_time(31_089_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	fn nudge_referendum_timed_out() -> Weight {
		// Minimum execution time: 22_203 nanoseconds.
		Weight::from_ref_time(23_408_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_deciding_failing() -> Weight {
		// Minimum execution time: 42_086 nanoseconds.
		Weight::from_ref_time(44_532_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Referenda DecidingCount (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_deciding_passing() -> Weight {
		// Minimum execution time: 44_254 nanoseconds.
		Weight::from_ref_time(45_996_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_confirming() -> Weight {
		// Minimum execution time: 40_864 nanoseconds.
		Weight::from_ref_time(42_518_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_end_confirming() -> Weight {
		// Minimum execution time: 41_997 nanoseconds.
		Weight::from_ref_time(43_555_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_continue_not_confirming() -> Weight {
		// Minimum execution time: 39_163 nanoseconds.
		Weight::from_ref_time(40_633_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_continue_confirming() -> Weight {
		// Minimum execution time: 38_260 nanoseconds.
		Weight::from_ref_time(39_494_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	// Storage: Scheduler Lookup (r:1 w:1)
	fn nudge_referendum_approved() -> Weight {
		// Minimum execution time: 51_267 nanoseconds.
		Weight::from_ref_time(54_875_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: Referenda ReferendumInfoFor (r:1 w:1)
	// Storage: Balances InactiveIssuance (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_rejected() -> Weight {
		// Minimum execution time: 41_099 nanoseconds.
		Weight::from_ref_time(43_408_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	fn set_some_metadata() -> Weight {
		Weight::from_parts(20_490_000, 5407)
			.saturating_add(T::DbWeight::get().reads(2_u64))
			.saturating_add(T::DbWeight::get().writes(1_u64))
	}
	fn clear_metadata() -> Weight {
		Weight::from_parts(19_917_000, 5368)
			.saturating_add(T::DbWeight::get().reads(2_u64))
			.saturating_add(T::DbWeight::get().writes(1_u64))
	}
}
