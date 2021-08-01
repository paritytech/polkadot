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
use xcm::{
	opaque::v0::prelude::*,
	v0::{Junction, Xcm},
};

#[test]
fn send_reserve_asset_deposit_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
		let sender: MultiLocation =
			Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm::ReserveAssetDeposit {
			assets: vec![ConcreteFungible { id: Parent.into(), amount }],
			effects: vec![
				buy_execution(weight),
				DepositAsset { assets: vec![All], dest: sender.clone() },
			],
		};
		assert_ok!(XcmPallet::send(Origin::signed(ALICE), RelayLocation::get(), message.clone()));
		assert_eq!(
			sent_xcm(),
			vec![(
				MultiLocation::Null,
				RelayedFrom { who: sender.clone(), message: Box::new(message.clone()) }
			)]
		);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Sent(sender, RelayLocation::get(), message))
		);
	});
}

#[test]
fn send_fails_when_xcm_router_blocks() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
		let sender: MultiLocation =
			Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm::ReserveAssetDeposit {
			assets: vec![ConcreteFungible { id: Parent.into(), amount }],
			effects: vec![
				buy_execution(weight),
				DepositAsset { assets: vec![All], dest: sender.clone() },
			],
		};
		assert_noop!(
			XcmPallet::send(
				Origin::signed(ALICE),
				X3(Junction::Parent, Junction::Parent, Junction::Parent),
				message.clone()
			),
			crate::Error::<Test>::SendFailure
		);
	});
}

#[test]
fn teleport_assets_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::teleport_assets(
			Origin::signed(ALICE),
			RelayLocation::get(),
			RelayLocation::get(),
			vec![ConcreteFungible { id: MultiLocation::Null, amount }],
			weight,
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - amount);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

#[test]
fn reserve_transfer_assets_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			dest.clone(),
			vec![ConcreteFungible { id: MultiLocation::Null, amount }],
			weight
		));
		// Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
		// Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + amount);
		assert_eq!(
			sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: vec![buy_execution(weight), DepositAsset { assets: vec![All], dest },]
				}
			)]
		);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

#[test]
fn execute_withdraw_to_teleport_works() {
	let balances =
		vec![(ALICE, INITIAL_BALANCE), (ParaId::from(PARA_ID).into_account(), INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 3 * BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		let teleport_effects = vec![
			buy_execution(0),
			Order::DepositAsset { assets: vec![All], dest: Parachain(PARA_ID).into() },
		];
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::execute(
			Origin::signed(ALICE),
			Box::new(Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: MultiLocation::Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::InitiateTeleport {
						assets: vec![All],
						dest,
						effects: teleport_effects.clone(),
					},
				],
			}),
			weight
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - amount);
		assert_eq!(
			last_event(),
			Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}
