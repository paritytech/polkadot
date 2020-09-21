// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Tests to make sure that Polkadot's weights and fees match what we
//! expect from Substrate.
//!
//! These test are not meant to be exhaustive, as it is inevitable that
//! weights in Substrate will change. Instead they are supposed to provide
//! some sort of indicator that calls we consider important (e.g pallet_balances::transfer)
//! have not suddenly changed from under us.
//!
//! Some of the tests in this crate print insightful logs. Run with:
//!
//! ```
//! $ cargo test -p polkadot-runtime -- --nocapture --test-threads=1
//! ```

use frame_support::weights::{constants::*, GetDispatchInfo};
use polkadot_runtime::{self, Runtime};
use runtime_common::MaximumBlockWeight;

use pallet_elections_phragmen::Call as PhragmenCall;
use frame_system::Call as SystemCall;

type DbWeight = <Runtime as frame_system::Trait>::DbWeight;

#[test]
fn sanity_check_weight_per_time_constants_are_as_expected() {
	// These values comes from Substrate, we want to make sure that if it
	// ever changes we don't accidentally break Polkadot
	assert_eq!(WEIGHT_PER_SECOND, 1_000_000_000_000);
	assert_eq!(WEIGHT_PER_MILLIS, WEIGHT_PER_SECOND / 1000);
	assert_eq!(WEIGHT_PER_MICROS, WEIGHT_PER_MILLIS / 1000);
	assert_eq!(WEIGHT_PER_NANOS, WEIGHT_PER_MICROS / 1000);
}

#[test]
fn weight_of_system_set_code_is_correct() {
	// #[weight = (T::MaximumBlockWeight::get(), DispatchClass::Operational)]
	let expected_weight = MaximumBlockWeight::get();
	let weight = SystemCall::set_code::<Runtime>(vec![]).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_vote_is_correct() {
	// (316_492_000 as Weight)
	// 	.saturating_add((440_000 as Weight).saturating_mul(v as Weight))
	// 	.saturating_add(DbWeight::get().reads(5 as Weight))
	// 	.saturating_add(DbWeight::get().writes(2 as Weight))
	let expected_weight =
		316_492_000 +
		(DbWeight::get().read * 5) + (DbWeight::get().write * 2) +
		440_000 * 2;
	let weight = PhragmenCall::vote::<Runtime>(
		vec![Default::default(), Default::default()],
		Default::default(),
	).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_submit_candidacy_is_correct() {
	// (341_953_000 as Weight)
	// 	.saturating_add((13_693_000 as Weight).saturating_mul(c as Weight))
	// 	.saturating_add(DbWeight::get().reads(3 as Weight))
	// 	.saturating_add(DbWeight::get().writes(1 as Weight))
	let expected_weight =
		341_953_000 +
		(13_693_000 * 3) +
		(DbWeight::get().read * 3) + (DbWeight::get().write * 1);
	let weight = PhragmenCall::submit_candidacy::<Runtime>(3).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_renounce_candidacy_member_is_correct() {
	// (389_571_000 as Weight)
	// 	.saturating_add(DbWeight::get().reads(3 as Weight))
	// 	.saturating_add(DbWeight::get().writes(4 as Weight))
	let expected_weight = 389_571_000 + (DbWeight::get().read * 3) + (DbWeight::get().write * 4);
	let weight = PhragmenCall::renounce_candidacy::<Runtime>(pallet_elections_phragmen::Renouncing::Member)
		.get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}
