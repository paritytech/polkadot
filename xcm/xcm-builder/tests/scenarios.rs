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

mod mock;

use mock::{
	kusama_like_with_balances, AccountId, Balance, Balances, BaseXcmWeight, System, XcmConfig,
	CENTS,
};
use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::AccountIdConversion;
use xcm::latest::prelude::*;
use xcm_executor::XcmExecutor;

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;
pub const REGISTER_AMOUNT: Balance = 10 * CENTS;

// Construct a `BuyExecution` order.
fn buy_execution<C>() -> Instruction<C> {
	BuyExecution { fees: (Here, REGISTER_AMOUNT).into(), weight_limit: Unlimited }
}

/// Scenario:
/// A parachain transfers funds on the relaychain to another parachain's account.
///
/// Asserts that the parachain accounts are updated as expected.
#[test]
fn withdraw_and_deposit_works() {
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount = REGISTER_AMOUNT;
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				DepositAsset {
					assets: All.into(),
					max_assets: 1,
					beneficiary: Parachain(other_para_id).into(),
				},
			]),
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account_truncating();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(Balances::free_balance(other_para_acc), amount);
	});
}

/// Scenario:
/// Alice simply wants to transfer funds to Bob's account via XCM.
///
/// Asserts that the balances are updated correctly and the correct events are fired.
#[test]
fn transfer_asset_works() {
	let bob = AccountId::new([1u8; 32]);
	let balances = vec![(ALICE, INITIAL_BALANCE), (bob.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let amount = REGISTER_AMOUNT;
		let weight = BaseXcmWeight::get();
		// Use `execute_xcm_in_credit` here to pass through the barrier
		let r = XcmExecutor::<XcmConfig>::execute_xcm_in_credit(
			AccountId32 { network: NetworkId::Any, id: ALICE.into() },
			Xcm(vec![TransferAsset {
				assets: (Here, amount).into(),
				beneficiary: AccountId32 { network: NetworkId::Any, id: bob.clone().into() }.into(),
			}]),
			weight,
			weight,
		);
		System::assert_last_event(
			pallet_balances::Event::Transfer { from: ALICE, to: bob.clone(), amount }.into(),
		);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
		assert_eq!(Balances::free_balance(bob), INITIAL_BALANCE + amount);
	});
}

/// Scenario:
/// A parachain wants to be notified that a transfer worked correctly.
/// It includes a `QueryHolding` order after the deposit to get notified on success.
/// This somewhat abuses `QueryHolding` as an indication of execution success. It works because
/// order execution halts on error (so no `QueryResponse` will be sent if the previous order failed).
/// The inner response sent due to the query is not used.
///
/// Asserts that the balances are updated correctly and the expected XCM is sent.
#[test]
fn query_holding_works() {
	use xcm::opaque::latest::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount = REGISTER_AMOUNT;
		let query_id = 1234;
		let weight = 4 * BaseXcmWeight::get();
		let max_response_weight = 1_000_000_000;
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				DepositAsset {
					assets: All.into(),
					max_assets: 1,
					beneficiary: OnlyChild.into(), // invalid destination
				},
				// is not triggered becasue the deposit fails
				QueryHolding {
					query_id,
					dest: Parachain(PARA_ID).into(),
					assets: All.into(),
					max_response_weight,
				},
			]),
			weight,
		);
		assert_eq!(
			r,
			Outcome::Incomplete(
				weight - BaseXcmWeight::get(),
				XcmError::FailedToTransactAsset("AccountIdConversionFailed")
			)
		);
		// there should be no query response sent for the failed deposit
		assert_eq!(mock::sent_xcm(), vec![]);
		assert_eq!(Balances::free_balance(para_acc.clone()), INITIAL_BALANCE - amount);

		// now do a successful transfer
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				DepositAsset {
					assets: All.into(),
					max_assets: 1,
					beneficiary: Parachain(other_para_id).into(),
				},
				// used to get a notification in case of success
				QueryHolding {
					query_id,
					dest: Parachain(PARA_ID).into(),
					assets: All.into(),
					max_response_weight: 1_000_000_000,
				},
			]),
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account_truncating();
		assert_eq!(Balances::free_balance(other_para_acc), amount);
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - 2 * amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm(vec![QueryResponse {
					query_id,
					response: Response::Assets(vec![].into()),
					max_weight: 1_000_000_000,
				}]),
			)]
		);
	});
}

/// Scenario:
/// A parachain wants to move KSM from Kusama to Statemine.
/// The parachain sends an XCM to withdraw funds combined with a teleport to the destination.
///
/// This way of moving funds from a relay to a parachain will only work for trusted chains.
/// Reserve based transfer should be used to move KSM to a community parachain.
///
/// Asserts that the balances are updated accordingly and the correct XCM is sent.
#[test]
fn teleport_to_statemine_works() {
	use xcm::opaque::latest::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let statemine_id = 1000;
		let other_para_id = 3000;
		let amount = REGISTER_AMOUNT;
		let teleport_effects = vec![
			buy_execution(), // unchecked mock value
			DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: (1, Parachain(PARA_ID)).into(),
			},
		];
		let weight = 3 * BaseXcmWeight::get();

		// teleports are allowed to community chains, even in the absence of trust from their side.
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				InitiateTeleport {
					assets: All.into(),
					dest: Parachain(other_para_id).into(),
					xcm: Xcm(teleport_effects.clone()),
				},
			]),
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(other_para_id).into(),
				Xcm(vec![ReceiveTeleportedAsset((Parent, amount).into()), ClearOrigin,]
					.into_iter()
					.chain(teleport_effects.clone().into_iter())
					.collect())
			)]
		);

		// teleports are allowed from statemine to kusama.
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				InitiateTeleport {
					assets: All.into(),
					dest: Parachain(statemine_id).into(),
					xcm: Xcm(teleport_effects.clone()),
				},
			]),
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		// 2 * amount because of the other teleport above
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - 2 * amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![
				(
					Parachain(other_para_id).into(),
					Xcm(vec![ReceiveTeleportedAsset((Parent, amount).into()), ClearOrigin,]
						.into_iter()
						.chain(teleport_effects.clone().into_iter())
						.collect()),
				),
				(
					Parachain(statemine_id).into(),
					Xcm(vec![ReceiveTeleportedAsset((Parent, amount).into()), ClearOrigin,]
						.into_iter()
						.chain(teleport_effects.clone().into_iter())
						.collect()),
				)
			]
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
	use xcm::opaque::latest::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount = REGISTER_AMOUNT;
		let transfer_effects = vec![
			buy_execution(), // unchecked mock value
			DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: (1, Parachain(PARA_ID)).into(),
			},
		];
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![
				WithdrawAsset((Here, amount).into()),
				buy_execution(),
				DepositReserveAsset {
					assets: All.into(),
					max_assets: 1,
					dest: Parachain(other_para_id).into(),
					xcm: Xcm(transfer_effects.clone()),
				},
			]),
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(other_para_id).into(),
				Xcm(vec![ReserveAssetDeposited((Parent, amount).into()), ClearOrigin,]
					.into_iter()
					.chain(transfer_effects.into_iter())
					.collect())
			)]
		);
	});
}
