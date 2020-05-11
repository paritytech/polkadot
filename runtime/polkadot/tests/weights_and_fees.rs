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

//! Tests to make sure that Polkadot's weights and fees match what we
//! expect from Substrate.
//!
//! NOTE: All the tests assume RocksDB as the RuntimeDbWeight type
//! which gives us the following weights:
//!  - Read: 25 * WEIGHT_PER_MICROS = 25 * 100_000_000,
//!  - Write: 100 * WEIGHT_PER_MICROS = 25 * 100_000_000,

use frame_support::weights::{GetDispatchInfo, constants::*};
use keyring::AccountKeyring;
use primitives::AccountId;
use polkadot_runtime::{self, Runtime};
use polkadot_runtime::constants::{currency::*, fee::*};
use staking::Call as StakingCall;

#[test]
fn sanity_check_weight_per_second_is_as_expected() {
	// This value comes from Substrate, we want to make sure that if it
	// ever changes we don't accidently break Polkadot
	assert_eq!(WEIGHT_PER_SECOND, 1_000_000_000_000)
}

#[test]
fn sanity_check_weight_per_milli_is_as_expected() {
	// This value comes from Substrate, we want to make sure that if it
	// ever changes we don't accidently break Polkadot
	assert_eq!(WEIGHT_PER_MILLIS, 1_000_000_000)
}

#[test]
fn sanity_check_weight_per_micros_is_as_expected() {
	// This value comes from Substrate, we want to make sure that if it
	// ever changes we don't accidently break Polkadot
	assert_eq!(WEIGHT_PER_MICROS, 1_000_000)
}

#[test]
fn sanity_check_weight_per_nanos_is_as_expected() {
	// This value comes from Substrate, we want to make sure that if it
	// ever changes we don't accidently break Polkadot
	assert_eq!(WEIGHT_PER_NANOS, 1_000)
}

#[test]
fn weight_of_transfer_is_correct() {
	let alice: AccountId = AccountKeyring::Alice.into();

	// #[weight = T::DbWeight::get().reads_writes(1, 1) + 70_000_000]
	let expected_weight = 195_000_000;

	let weight = polkadot_runtime::BalancesCall::transfer::<Runtime>(alice, 42 * DOLLARS).get_dispatch_info().weight;
	assert_eq!(weight, expected_weight);
}

#[test]
fn transfer_fees_are_correct() {
	use sp_runtime::traits::Convert;

	let alice: AccountId = AccountKeyring::Alice.into();

	let expected_fee = 15_600_000;

	let weight = polkadot_runtime::BalancesCall::transfer::<Runtime>(alice, 42 * DOLLARS).get_dispatch_info().weight;
	let fee = WeightToFee::convert(weight);
	assert_eq!(fee, expected_fee);
}

#[test]
fn weight_of_set_balance_is_correct() {
	let alice: AccountId = AccountKeyring::Alice.into();

	// #[weight = T::DbWeight::get().reads_writes(1, 1) + 35_000_000]
	let expected_weight = 160_000_000;

	let weight =
	polkadot_runtime::BalancesCall::set_balance::<Runtime>(alice, 12 * DOLLARS, 34 * DOLLARS)
		.get_dispatch_info()
		.weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_force_transfer_is_correct() {
	let alice: AccountId = AccountKeyring::Alice.into();
	let bob: AccountId = AccountKeyring::Alice.into();

	// #[weight = T::DbWeight::get().reads_writes(2, 2) + 70_000_000]
	let expected_weight = 320_000_000;

	let weight =
	polkadot_runtime::BalancesCall::force_transfer::<Runtime>(alice, bob, 34 * DOLLARS)
		.get_dispatch_info()
		.weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_transfer_keep_alive_is_correct() {
	let alice: AccountId = AccountKeyring::Alice.into();

	// #[weight = T::DbWeight::get().reads_writes(1, 1) + 50_000_000]
	let expected_weight = 175_000_000;

	let weight =
	polkadot_runtime::BalancesCall::transfer_keep_alive::<Runtime>(alice, 42 * DOLLARS)
		.get_dispatch_info()
		.weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_set_timestamp_is_correct() {
	// #[weight = T::DbWeight::get().reads_writes(2, 1) + 9_000_000]
	let expected_weight = 159_000_000;
	let weight = polkadot_runtime::TimestampCall::set::<Runtime>(1234).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_bond_is_correct() {
	let controller: AccountId = AccountKeyring::Alice.into();

	// #[weight = 500_000_000]
	let expected_weight = 500_000_000;
	let weight = StakingCall::bond::<Runtime>(controller, 1 * DOLLARS, Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_bond_extra_is_correct() {
	// #[weight = 500_000_000]
	let expected_weight = 500_000_000;
	let weight = StakingCall::bond_extra::<Runtime>(1 * DOLLARS).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_unbond_is_correct() {
	// #[weight = 400_000_000]
	let expected_weight = 400_000_000;
	let weight = StakingCall::unbond::<Runtime>(Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_widthdraw_unbonded_is_correct() {
	// #[weight = 400_000_000]
	let expected_weight = 400_000_000;
	let weight = StakingCall::withdraw_unbonded::<Runtime>().get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_validate_is_correct() {
	// #[weight = 750_000_000]
	let expected_weight = 750_000_000;
	let weight = StakingCall::validate::<Runtime>(Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_nominate_is_correct() {
	// #[weight = 750_000_000]
	let expected_weight = 750_000_000;
	let weight = StakingCall::nominate::<Runtime>(vec![]).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}
