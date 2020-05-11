// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Tests for Polkadot's weight and fee mechanics

use frame_support::weights::{GetDispatchInfo, constants::WEIGHT_PER_SECOND};
use primitives::AccountId;
use polkadot_runtime_test_client::{self, prelude::*, runtime};
use polkadot_runtime::Runtime;
use polkadot_runtime::constants::{currency::*, fee::*};

#[test]
// Sanity check to make sure that the weight value here is what we expect from Substrate.
fn sanity_check_weight_is_as_expected() {
	assert_eq!(WEIGHT_PER_SECOND, 1_000_000_000_000)
}

#[test]
fn weight_of_transfer_is_correct() {
	let alice: AccountId = AccountKeyring::Alice.into();
	let bob: AccountId = AccountKeyring::Bob.into();

	let expected_weight = 195_000_000;

	let weight = runtime::BalancesCall::transfer::<Runtime>(bob, 42 * DOLLARS).get_dispatch_info().weight;
	assert_eq!(weight, expected_weight);
}

#[test]
fn transfer_fees_are_correct() {
	use sp_runtime::traits::Convert;

	let alice: AccountId = AccountKeyring::Alice.into();
	let bob: AccountId = AccountKeyring::Bob.into();

	let expected_fee = 15_600_000;

	let weight = runtime::BalancesCall::transfer::<Runtime>(bob, 42 * DOLLARS).get_dispatch_info().weight;
	let fee = WeightToFee::convert(weight);
	assert_eq!(fee, expected_fee);
}
