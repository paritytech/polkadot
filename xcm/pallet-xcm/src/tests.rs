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

use crate::mock::*;
use frame_support::{assert_noop, assert_ok, traits::Currency};
use polkadot_parachain::primitives::{AccountIdConversion, Id as ParaId};
use std::convert::TryInto;
use xcm::latest::prelude::*;

const ALICE: AccountId = AccountId::new([0u8; 32]);
const BOB: AccountId = AccountId::new([1u8; 32]);
const PARA_ID: u32 = 2000;
const INITIAL_BALANCE: u128 = 100;
const SEND_AMOUNT: u128 = 10;

/// Test sending an `XCM` message (`XCM::ReserveAssetDeposit`)
///
/// Asserts that the expected message is sent and the event is emitted
#[test]
fn send_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let sender: MultiLocation =
			AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm(vec![
			ReserveAssetDeposited { assets: (Parent, SEND_AMOUNT).into() },
			ClearOrigin,
			buy_execution((Parent, SEND_AMOUNT)),
			DepositAsset { assets: All.into(), max_assets: 1, beneficiary: sender.clone() },
		]);
		assert_ok!(XcmPallet::send(Origin::signed(ALICE), RelayLocation::get(), message.clone(),));
		assert_eq!(
			sent_xcm(),
			vec![(Here.into(), Xcm(vec![DescendOrigin(sender.clone().try_into().unwrap())]))],
		);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Sent(sender, RelayLocation::get(), message))
		);
	});
}

/// Test that sending an `XCM` message fails when the `XcmRouter` blocks the
/// matching message format
///
/// Asserts that `send` fails with `Error::SendFailure`
#[test]
fn send_fails_when_xcm_router_blocks() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let sender: MultiLocation =
			Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm(vec![
			ReserveAssetDeposited { assets: (Parent, SEND_AMOUNT).into() },
			buy_execution((Parent, SEND_AMOUNT)),
			DepositAsset { assets: All.into(), max_assets: 1, beneficiary: sender.clone() },
		]);
		assert_noop!(
			XcmPallet::send(Origin::signed(ALICE), MultiLocation::ancestor(8), message,),
			crate::Error::<Test>::SendFailure,
		);
	});
}

/// Test `teleport_assets`
///
/// Asserts that the sender's balance is decreased as a result of execution of
/// local effects.
#[test]
fn teleport_assets_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 2 * BaseXcmWeight::get();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::teleport_assets(
			Origin::signed(ALICE),
			Box::new(RelayLocation::get()),
			Box::new(AccountId32 { network: Any, id: BOB.into() }.into()),
			(Here, SEND_AMOUNT).into(),
			0,
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `reserve_transfer_assets`
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the correct message is sent and event is emitted.
#[test]
fn reserve_transfer_assets_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Box::new(Parachain(PARA_ID).into()),
			Box::new(dest.clone()),
			(Here, SEND_AMOUNT).into(),
			0,
		));
		// Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		// Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm(vec![
					ReserveAssetDeposited { assets: (Parent, SEND_AMOUNT).into() },
					buy_execution((Parent, SEND_AMOUNT)),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test local execution of XCM
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the expected event is emitted.
#[test]
fn execute_withdraw_to_deposit_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 3 * BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: BOB.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::execute(
			Origin::signed(ALICE),
			Xcm(vec![
				WithdrawAsset { assets: (Here, SEND_AMOUNT).into() },
				buy_execution((Here, SEND_AMOUNT)),
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
			]),
			weight
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(Balances::total_balance(&BOB), SEND_AMOUNT);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}
