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
//! some sort of indicator that calls we consider important (e.g Balances::transfer)
//! have not suddenly changed from under us.

use frame_support::{
	traits::ContainsLengthBound,
	weights::{constants::*, GetDispatchInfo, Weight},
};
use keyring::AccountKeyring;
use polkadot_runtime::constants::currency::*;
use polkadot_runtime::{self, Runtime};
use primitives::AccountId;
use runtime_common::MaximumBlockWeight;

use democracy::Call as DemocracyCall;
use elections_phragmen::Call as PhragmenCall;
use session::Call as SessionCall;
use staking::Call as StakingCall;
use system::Call as SystemCall;
use treasury::Call as TreasuryCall;

type DbWeight = <Runtime as system::Trait>::DbWeight;

#[test]
fn sanity_check_weight_per_time_constants_are_as_expected() {
	// These values comes from Substrate, we want to make sure that if it
	// ever changes we don't accidently break Polkadot
	assert_eq!(WEIGHT_PER_SECOND, 1_000_000_000_000);
	assert_eq!(WEIGHT_PER_MILLIS, WEIGHT_PER_SECOND / 1000);
	assert_eq!(WEIGHT_PER_MICROS, WEIGHT_PER_MILLIS / 1000);
	assert_eq!(WEIGHT_PER_NANOS, WEIGHT_PER_MICROS / 1000);
}

#[test]
fn weight_of_balances_transfer_is_correct() {
	// #[weight = T::DbWeight::get().reads_writes(1, 1) + 70_000_000]
	let expected_weight = DbWeight::get().read + DbWeight::get().write + 70_000_000;

	let weight = polkadot_runtime::BalancesCall::transfer::<Runtime>(Default::default(), Default::default())
		.get_dispatch_info()
		.weight;
	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_balances_transfer_keep_alive_is_correct() {
	// #[weight = T::DbWeight::get().reads_writes(1, 1) + 50_000_000]
	let expected_weight = DbWeight::get().read + DbWeight::get().write + 50_000_000;

	let weight = polkadot_runtime::BalancesCall::transfer_keep_alive::<Runtime>(Default::default(), Default::default())
		.get_dispatch_info()
		.weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_timestamp_set_is_correct() {
	// #[weight = T::DbWeight::get().reads_writes(2, 1) + 8_000_000]
	let expected_weight = (2 * DbWeight::get().read) + DbWeight::get().write + 8_000_000;
	let weight = polkadot_runtime::TimestampCall::set::<Runtime>(Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_bond_is_correct() {
	let controller: AccountId = AccountKeyring::Alice.into();

	// #[weight = 67 * WEIGHT_PER_MICROS + T::DbWeight::get().reads_writes(5, 4)]
	let expected_weight = 67 * WEIGHT_PER_MICROS + (DbWeight::get().read * 5) + (DbWeight::get().write * 4);
	let weight = StakingCall::bond::<Runtime>(controller, 1 * DOLLARS, Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_validate_is_correct() {
	// #[weight = 17 * WEIGHT_PER_MICROS + T::DbWeight::get().reads_writes(2, 2)]
	let expected_weight = 17 * WEIGHT_PER_MICROS + (DbWeight::get().read * 2) + (DbWeight::get().write * 2);
	let weight = StakingCall::validate::<Runtime>(Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_staking_nominate_is_correct() {
	let targets: Vec<AccountId> = vec![Default::default(), Default::default(), Default::default()];

	// #[weight = T::DbWeight::get().reads_writes(3, 2)
	// 	.saturating_add(22 * WEIGHT_PER_MICROS)
	// 	.saturating_add((360 * WEIGHT_PER_NANOS).saturating_mul(targets.len() as Weight))
	// ]
	let db_weight = (DbWeight::get().read * 3) + (DbWeight::get().write * 2);
	let targets_weight = (360 * WEIGHT_PER_NANOS).saturating_mul(targets.len() as Weight);

	let expected_weight = db_weight.saturating_add(22 * WEIGHT_PER_MICROS).saturating_add(targets_weight);
	let weight = StakingCall::nominate::<Runtime>(targets).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_system_set_code_is_correct() {
	// #[weight = (T::MaximumBlockWeight::get(), DispatchClass::Operational)]
	let expected_weight = MaximumBlockWeight::get();
	let weight = SystemCall::set_code::<Runtime>(vec![]).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_system_set_storage_is_correct() {
	let storage_items = vec![(vec![12], vec![34]), (vec![45], vec![83])];
	let len = storage_items.len() as Weight;

	// #[weight = FunctionOf(
	// 	|(items,): (&Vec<KeyValue>,)| {
	// 		T::DbWeight::get().writes(items.len() as Weight)
	// 			.saturating_add((items.len() as Weight).saturating_mul(600_000))
	// 	},
	// 	DispatchClass::Operational,
	// 	Pays::Yes,
	// )]
	let expected_weight = (DbWeight::get().write * len).saturating_add(len.saturating_mul(600_000));
	let weight = SystemCall::set_storage::<Runtime>(storage_items).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_system_remark_is_correct() {
	// #[weight = 700_000]
	let expected_weight = 700_000;
	let weight = SystemCall::remark::<Runtime>(vec![]).get_dispatch_info().weight;

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
fn weight_of_democracy_propose_is_correct() {
	// #[weight = 50_000_000 + T::DbWeight::get().reads_writes(2, 3)]
	let expected_weight = 50_000_000 + (DbWeight::get().read * 2) + (DbWeight::get().write * 3);
	let weight = DemocracyCall::propose::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_democracy_vote_is_correct() {
	use democracy::AccountVote;
	let vote = AccountVote::Standard { vote: Default::default(), balance: Default::default() };

	// #[weight = 50_000_000 + 350_000 * Weight::from(T::MaxVotes::get()) + T::DbWeight::get().reads_writes(3, 3)]
	let expected_weight = 50_000_000
		+ 350_000 * (Weight::from(polkadot_runtime::MaxVotes::get()))
		+ (DbWeight::get().read * 3)
		+ (DbWeight::get().write * 3);
	let weight = DemocracyCall::vote::<Runtime>(Default::default(), vote).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}

#[test]
fn weight_of_democracy_enact_proposal_is_correct() {
	// #[weight = T::MaximumBlockWeight::get()]
	let expected_weight = MaximumBlockWeight::get();
	let weight =
		DemocracyCall::enact_proposal::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

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
	let weight = PhragmenCall::renounce_candidacy::<Runtime>(elections_phragmen::Renouncing::Member)
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
	let max_len: Weight = <Runtime as treasury::Trait>::Tippers::max_len() as Weight;

	// #[weight = 68_000_000 + 2_000_000 * T::Tippers::max_len() as Weight
	// 	+ T::DbWeight::get().reads_writes(2, 1)]
	let expected_weight = 68_000_000 + 2_000_000 * max_len + 2 * DbWeight::get().read + DbWeight::get().write;
	let weight = TreasuryCall::tip::<Runtime>(Default::default(), Default::default()).get_dispatch_info().weight;

	assert_eq!(weight, expected_weight);
}
