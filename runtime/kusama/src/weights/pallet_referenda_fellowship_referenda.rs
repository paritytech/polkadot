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
	// Storage: FellowshipCollective Members (r:1 w:0)
	// Storage: FellowshipReferenda ReferendumCount (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	// Storage: FellowshipReferenda ReferendumInfoFor (r:0 w:1)
	fn submit() -> Weight {
		// Minimum execution time: 33_277 nanoseconds.
		Weight::from_ref_time(33_960_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn place_decision_deposit_preparing() -> Weight {
		// Minimum execution time: 53_029 nanoseconds.
		Weight::from_ref_time(54_668_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:0)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	fn place_decision_deposit_queued() -> Weight {
		// Minimum execution time: 90_942 nanoseconds.
		Weight::from_ref_time(102_908_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:0)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	fn place_decision_deposit_not_queued() -> Weight {
		// Minimum execution time: 93_418 nanoseconds.
		Weight::from_ref_time(102_043_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn place_decision_deposit_passing() -> Weight {
		// Minimum execution time: 204_407 nanoseconds.
		Weight::from_ref_time(221_043_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	fn place_decision_deposit_failing() -> Weight {
		// Minimum execution time: 46_345 nanoseconds.
		Weight::from_ref_time(48_000_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	fn refund_decision_deposit() -> Weight {
		// Minimum execution time: 32_528 nanoseconds.
		Weight::from_ref_time(33_923_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	fn refund_submission_deposit() -> Weight {
		// Minimum execution time: 21_065 nanoseconds.
		Weight::from_ref_time(21_947_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn cancel() -> Weight {
		// Minimum execution time: 41_541 nanoseconds.
		Weight::from_ref_time(43_091_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn kill() -> Weight {
		// Minimum execution time: 75_008 nanoseconds.
		Weight::from_ref_time(77_437_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda TrackQueue (r:1 w:0)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:1)
	fn one_fewer_deciding_queue_empty() -> Weight {
		// Minimum execution time: 10_620 nanoseconds.
		Weight::from_ref_time(11_309_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn one_fewer_deciding_failing() -> Weight {
		// Minimum execution time: 167_988 nanoseconds.
		Weight::from_ref_time(179_043_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	fn one_fewer_deciding_passing() -> Weight {
		// Minimum execution time: 171_829 nanoseconds.
		Weight::from_ref_time(183_097_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_requeued_insertion() -> Weight {
		// Minimum execution time: 99_225 nanoseconds.
		Weight::from_ref_time(109_234_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_requeued_slide() -> Weight {
		// Minimum execution time: 100_605 nanoseconds.
		Weight::from_ref_time(109_740_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:0)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_queued() -> Weight {
		// Minimum execution time: 105_414 nanoseconds.
		Weight::from_ref_time(112_810_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:0)
	// Storage: FellowshipReferenda TrackQueue (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:0)
	fn nudge_referendum_not_queued() -> Weight {
		// Minimum execution time: 100_969 nanoseconds.
		Weight::from_ref_time(114_514_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_no_deposit() -> Weight {
		// Minimum execution time: 29_982 nanoseconds.
		Weight::from_ref_time(31_303_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_preparing() -> Weight {
		// Minimum execution time: 30_756 nanoseconds.
		Weight::from_ref_time(31_875_000)
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	fn nudge_referendum_timed_out() -> Weight {
		// Minimum execution time: 22_922 nanoseconds.
		Weight::from_ref_time(23_811_000)
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_deciding_failing() -> Weight {
		// Minimum execution time: 42_030 nanoseconds.
		Weight::from_ref_time(43_505_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipReferenda DecidingCount (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_deciding_passing() -> Weight {
		// Minimum execution time: 95_523 nanoseconds.
		Weight::from_ref_time(102_104_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_begin_confirming() -> Weight {
		// Minimum execution time: 166_858 nanoseconds.
		Weight::from_ref_time(190_720_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_end_confirming() -> Weight {
		// Minimum execution time: 165_354 nanoseconds.
		Weight::from_ref_time(185_408_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_continue_not_confirming() -> Weight {
		// Minimum execution time: 167_797 nanoseconds.
		Weight::from_ref_time(182_086_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_continue_confirming() -> Weight {
		// Minimum execution time: 87_123 nanoseconds.
		Weight::from_ref_time(94_538_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:2 w:2)
	// Storage: Scheduler Lookup (r:1 w:1)
	fn nudge_referendum_approved() -> Weight {
		// Minimum execution time: 177_202 nanoseconds.
		Weight::from_ref_time(194_662_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: FellowshipReferenda ReferendumInfoFor (r:1 w:1)
	// Storage: FellowshipCollective MemberCount (r:1 w:0)
	// Storage: Scheduler Agenda (r:1 w:1)
	fn nudge_referendum_rejected() -> Weight {
		// Minimum execution time: 154_150 nanoseconds.
		Weight::from_ref_time(185_014_000)
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	fn set_some_metadata() -> Weight {
		Weight::from_parts(20_490_000, 5407)
			.saturating_add(RocksDbWeight::get().reads(2_u64))
			.saturating_add(RocksDbWeight::get().writes(1_u64))
	}
	fn clear_metadata() -> Weight {
		Weight::from_parts(19_917_000, 5368)
			.saturating_add(RocksDbWeight::get().reads(2_u64))
			.saturating_add(RocksDbWeight::get().writes(1_u64))
	}
}
