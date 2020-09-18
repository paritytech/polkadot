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

use codec::Encode;
use frame_support::{
	traits::ContainsLengthBound,
	weights::{constants::*, GetDispatchInfo, Weight, DispatchInfo},
};
use keyring::AccountKeyring;
use polkadot_runtime::constants::currency::*;
use polkadot_runtime::{self, Runtime};
use primitives::v0::AccountId;
use runtime_common::MaximumBlockWeight;

use pallet_elections_phragmen::Call as PhragmenCall;
use pallet_session::Call as SessionCall;
use pallet_staking::Call as StakingCall;
use frame_system::Call as SystemCall;
use pallet_treasury::Call as TreasuryCall;

type DbWeight = <Runtime as frame_system::Trait>::DbWeight;


fn report_portion(name: &'static str, info: DispatchInfo, len: usize) {
	let maximum_weight = <Runtime as frame_system::Trait>::MaximumBlockWeight::get();
	let fee = sp_io::TestExternalities::new(Default::default()).execute_with(|| {
		<pallet_transaction_payment::Module<Runtime>>::compute_fee(len as u32, &info, 0)
	});

	let portion = info.weight as f64 / maximum_weight as f64;

	if portion > 0.5 {
		panic!("Weight of some call seem to have exceeded half of the block. Probably something is wrong.");
	}

	println!("\nCall {} (with default args) takes {} of the block weight, pays {} in fee.", name, portion, fee);
}

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
fn weight_of_staking_bond_is_correct() {
	let controller: AccountId = AccountKeyring::Alice.into();

	// (144278000 as Weight)
	// 	.saturating_add(DbWeight::get().reads(5 as Weight))
	// 	.saturating_add(DbWeight::get().writes(4 as Weight))
	let expected_weight = 144278000 + (DbWeight::get().read * 5) + (DbWeight::get().write * 4);
	let call = StakingCall::bond::<Runtime>(controller, 1 * DOLLARS, Default::default());
	let info = call.get_dispatch_info();

	assert_eq!(info.weight, expected_weight);
	report_portion("staking_bond", info, call.encode().len())
}

#[test]
fn weight_of_staking_validate_is_correct() {
	// (35539000 as Weight)
	// 	.saturating_add(DbWeight::get().reads(2 as Weight))
	// 	.saturating_add(DbWeight::get().writes(2 as Weight))
	let expected_weight = 35539000 + (DbWeight::get().read * 2) + (DbWeight::get().write * 2);
	let call = StakingCall::validate::<Runtime>(Default::default());
	let info = call.get_dispatch_info();

	assert_eq!(info.weight, expected_weight);
	report_portion("staking_validate", info, call.encode().len())
}

#[test]
fn weight_of_staking_nominate_is_correct() {
	let targets: Vec<AccountId> = vec![Default::default(), Default::default(), Default::default()];

	// (48596000 as Weight)
	// 	.saturating_add((308000 as Weight).saturating_mul(n as Weight))
	// 	.saturating_add(DbWeight::get().reads(3 as Weight))
	// 	.saturating_add(DbWeight::get().writes(2 as Weight))
	let db_weight = (DbWeight::get().read * 3) + (DbWeight::get().write * 2);
	let targets_weight = (308 * WEIGHT_PER_NANOS).saturating_mul(targets.len() as Weight);

	let expected_weight = db_weight.saturating_add(48596000).saturating_add(targets_weight);
	let call = StakingCall::nominate::<Runtime>(targets);
	let info = call.get_dispatch_info();

	assert_eq!(info.weight, expected_weight);
	report_portion("staking_nominate", info, call.encode().len())
}

#[test]
fn weight_of_staking_payout_staker_is_correct() {
	// (0 as Weight)
	// 	.saturating_add((117324000 as Weight).saturating_mul(n as Weight))
	// 	.saturating_add(DbWeight::get().reads((5 as Weight).saturating_mul(n as Weight)))
	// 	.saturating_add(DbWeight::get().writes((3 as Weight).saturating_mul(n as Weight)))
	let call = StakingCall::payout_stakers::<Runtime>(Default::default(), 0u32);
	let info = call.get_dispatch_info();

	let n = <Runtime as pallet_staking::Trait>::MaxNominatorRewardedPerValidator::get() as Weight;
	let mut expected_weight = (117324000 as Weight).saturating_mul(n as Weight);
	expected_weight += (DbWeight::get().read * 5 * n) + (DbWeight::get().write * 3 * n);

	assert_eq!(info.weight, expected_weight);
	report_portion("staking_payout_stakers", info, call.encode().len())
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

#[test]
fn weight_of_treasury_propose_spend_is_correct() {
	// #[weight = 120_000_000 + T::DbWeight::get().reads_writes(1, 2)]
	let expected_weight = 120_000_000 + DbWeight::get().read + 2 * DbWeight::get().write;
	let weight =
		TreasuryCall::propose_spend::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_treasury_approve_proposal_is_correct() {
	// #[weight = (34_000_000 + T::DbWeight::get().reads_writes(2, 1), DispatchClass::Operational)]
	let expected_weight = 34_000_000 + 2 * DbWeight::get().read + DbWeight::get().write;
	let weight = TreasuryCall::approve_proposal::<Runtime>(Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_treasury_tip_is_correct() {
	let max_len: Weight = <Runtime as pallet_treasury::Trait>::Tippers::max_len() as Weight;

	// #[weight = 68_000_000 + 2_000_000 * T::Tippers::max_len() as Weight
	// 	+ T::DbWeight::get().reads_writes(2, 1)]
	let expected_weight = 68_000_000 + 2_000_000 * max_len + 2 * DbWeight::get().read + DbWeight::get().write;
	let weight = TreasuryCall::tip::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}
