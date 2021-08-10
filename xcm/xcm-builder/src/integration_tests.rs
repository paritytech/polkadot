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

use frame_support::weights::Weight;
use sp_runtime::traits::AccountIdConversion;
use polkadot_parachain::primitives::Id as ParaId;
use xcm::opaque::v0::prelude::*;
use xcm::opaque::v0::{Response};
use xcm::v0::{MultiLocation::*, Order};
use xcm_executor::XcmExecutor;
use crate::mock;
use crate::integration_mock::{
	AccountId, Balances, BaseXcmWeight, ExistentialDeposit, kusama_like_with_balances, XcmConfig
};

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;

// Construct a `BuyExecution` order.
fn buy_execution<C>(debt: Weight) -> Order<C> {
	use xcm::opaque::v0::prelude::*;
	Order::BuyExecution {
		fees: All,
		weight: 0,
		debt,
		halt_on_error: false,
		xcm: vec![],
	}
}

/// Scenario:
/// A parachain transfers funds on the relaychain to another parachain's account.
///
/// Asserts that the parachain accounts are updated as expected.
#[test]
fn withdraw_and_deposit_works() {
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount =  10 * ExistentialDeposit::get();
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(Balances::free_balance(other_para_acc), amount);
	});
}

/// Scenario:
/// A parachain wants to be notified that a transfer worked correctly.
/// It sends a `QueryHolding` after the deposit to get notified on success.
/// This somewhat abuses `QueryHolding` as an indication of execution success. It works because
/// order execution halts on error (so no `QueryResponse` will be sent if the previous order failed).
/// The inner response is not used.
///
/// Asserts that the balances are updated correctly and the expected XCM is sent.
#[test]
fn query_holding_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount = 10 * ExistentialDeposit::get();
		let query_id = 1234;
		let weight = 4 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: OnlyChild.into(), // invalid destination
					},
					// is not triggered becasue the deposit fails
					Order::QueryHolding {
						query_id,
						dest: Parachain(PARA_ID).into(),
						assets: vec![All],
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Incomplete(weight, XcmError::FailedToTransactAsset("AccountIdConversionFailed")));
		// there should be no query response sent for the failed deposit
		assert_eq!(mock::sent_xcm(), vec![]);
		assert_eq!(Balances::free_balance(para_acc.clone()), INITIAL_BALANCE - amount);

		// now do a successful transfer
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
					// used to get a notification in case of success
					Order::QueryHolding {
						query_id,
						dest: Parachain(PARA_ID).into(),
						assets: vec![All],
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(other_para_acc), amount);
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - 2 * amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::QueryResponse {
					query_id,
					response: Response::Assets(vec![])
				}
			)]
		);
	});
}

/// Scenario:
/// A parachain wants to move KSM from Kusama to Statemine.
/// It withdraws funds and then teleports them to the destination.
///
/// This way of moving funds from a relay to a parachain will only work for trusted chains.
/// Reserve based transfer should be used to move KSM to a community parachain.
///
/// Asserts that the balances are updated accordingly and the correct XCM is sent.
#[test]
fn teleport_to_statemine_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let statemine_id = 1000;
		let other_para_id = 3000;
		let amount =  10 * ExistentialDeposit::get();
		let teleport_effects = vec![
			buy_execution(5), // unchecked mock value
			Order::DepositAsset { assets: vec![All], dest: X2(Parent, Parachain(PARA_ID)) },
		];
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::InitiateTeleport {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
						effects: teleport_effects.clone(),
					},
				],
			},
			weight,
		);
		// teleports not allowed to community chains
		assert_eq!(r, Outcome::Incomplete(weight, XcmError::UntrustedTeleportLocation));
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::InitiateTeleport {
						assets: vec![All],
						dest: Parachain(statemine_id).into(),
						effects: teleport_effects.clone(),
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		// 2 * amount because of the incomplete failed attempt above
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - 2 * amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(statemine_id).into(),
				Xcm::TeleportAsset {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: teleport_effects,
				}
			)]
		);
	});
}

/// Scenario:
/// A parachain wants to move KSM from Kusama to the parachain.
/// It withdraws funds and then deposits them into the reserve account of the destination chain.
/// to the destination.
///
/// Asserts that the balances are updated accordingly and the correct XCM is sent.
#[test]
fn reserve_based_transfer_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount =  10 * ExistentialDeposit::get();
		let transfer_effects = vec![
			buy_execution(5), // unchecked mock value
			Order::DepositAsset { assets: vec![All], dest: X2(Parent, Parachain(PARA_ID)) },
		];
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositReserveAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
						effects: transfer_effects.clone(),
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(other_para_id).into(),
				Xcm::ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: transfer_effects,
				}
			)]
		);
	});
}
