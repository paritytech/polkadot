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
use pallet_session::Call as SessionCall;
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
fn weight_of_session_set_keys_is_correct() {
	// #[weight = 200_000_000
	// 	+ T::DbWeight::get().reads(2 + T::Keys::key_ids().len() as Weight)
	// 	+ T::DbWeight::get().writes(1 + T::Keys::key_ids().len() as Weight)]
	//
	// Polkadot has five possible session keys, so we default to key_ids.len() = 5
	let expected_weight = 200_000_000 + (DbWeight::get().read * (2 + 5)) + (DbWeight::get().write * (1 + 5));
	let weight = SessionCall::set_keys::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_session_purge_keys_is_correct() {
	// #[weight = 120_000_000
	// 	+ T::DbWeight::get().reads_writes(2, 1 + T::Keys::key_ids().len() as Weight)]
	//
	// Polkadot has five possible session keys, so we default to key_ids.len() = 5
	let expected_weight = 120_000_000 + (DbWeight::get().read * 2) + (DbWeight::get().write * (1 + 5));
	let weight = SessionCall::purge_keys::<Runtime>().get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_vote_is_correct() {
	// #[weight = 100_000_000]
	let expected_weight = 350_000_000;
	let weight = PhragmenCall::vote::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_submit_candidacy_is_correct() {
	let expected_weight = WEIGHT_PER_MICROS * 35 + 1 * 375 * WEIGHT_PER_NANOS + DbWeight::get().reads_writes(4, 1);
	let weight = PhragmenCall::submit_candidacy::<Runtime>(1).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_phragmen_renounce_candidacy_is_correct() {
	let expected_weight = 46 * WEIGHT_PER_MICROS + DbWeight::get().reads_writes(2, 2);
	let weight = PhragmenCall::renounce_candidacy::<Runtime>(pallet_elections_phragmen::Renouncing::Member)
		.get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}
