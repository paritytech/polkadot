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

use crate::{
	mock::*, AssetTraps, CurrentMigration, Error, LatestVersionedMultiLocation, Queries,
	QueryStatus, VersionDiscoveryQueue, VersionNotifiers, VersionNotifyTargets,
};
use frame_support::{
	assert_noop, assert_ok,
	traits::{Currency, Hooks},
	weights::Weight,
};
use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::{AccountIdConversion, BlakeTwo256, Hash};
use xcm::prelude::*;
use xcm_builder::AllowKnownQueryResponses;
use xcm_executor::{traits::ShouldExecute, XcmExecutor};

const ALICE: AccountId = AccountId::new([0u8; 32]);
const BOB: AccountId = AccountId::new([1u8; 32]);
const PARA_ID: u32 = 2000;
const INITIAL_BALANCE: u128 = 100;
const SEND_AMOUNT: u128 = 10;

#[test]
fn report_outcome_notify_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	let sender = AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
	let mut message = Xcm(vec![TransferAsset {
		assets: (Here, SEND_AMOUNT).into(),
		beneficiary: sender.clone(),
	}]);
	let call = pallet_test_notifier::Call::notification_received {
		query_id: 0,
		response: Default::default(),
	};
	let notify = RuntimeCall::TestNotifier(call);
	new_test_ext_with_balances(balances).execute_with(|| {
		XcmPallet::report_outcome_notify(&mut message, Parachain(PARA_ID).into(), notify, 100)
			.unwrap();
		assert_eq!(
			message,
			Xcm(vec![
				SetAppendix(Xcm(vec![ReportError {
					query_id: 0,
					dest: Parent.into(),
					max_response_weight: 1_000_000
				},])),
				TransferAsset { assets: (Here, SEND_AMOUNT).into(), beneficiary: sender.clone() },
			])
		);
		let status = QueryStatus::Pending {
			responder: MultiLocation::from(Parachain(PARA_ID)).into(),
			maybe_notify: Some((4, 2)),
			timeout: 100,
		};
		assert_eq!(crate::Queries::<Test>::iter().collect::<Vec<_>>(), vec![(0, status)]);

		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![QueryResponse {
				query_id: 0,
				response: Response::ExecutionResult(None),
				max_weight: 1_000_000,
			}]),
			1_000_000_000,
		);
		assert_eq!(r, Outcome::Complete(1_000));
		assert_eq!(
			last_events(2),
			vec![
				RuntimeEvent::TestNotifier(pallet_test_notifier::Event::ResponseReceived(
					Parachain(PARA_ID).into(),
					0,
					Response::ExecutionResult(None),
				)),
				RuntimeEvent::XcmPallet(crate::Event::Notified(0, 4, 2)),
			]
		);
		assert_eq!(crate::Queries::<Test>::iter().collect::<Vec<_>>(), vec![]);
	});
}

#[test]
fn report_outcome_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	let sender = AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
	let mut message = Xcm(vec![TransferAsset {
		assets: (Here, SEND_AMOUNT).into(),
		beneficiary: sender.clone(),
	}]);
	new_test_ext_with_balances(balances).execute_with(|| {
		XcmPallet::report_outcome(&mut message, Parachain(PARA_ID).into(), 100).unwrap();
		assert_eq!(
			message,
			Xcm(vec![
				SetAppendix(Xcm(vec![ReportError {
					query_id: 0,
					dest: Parent.into(),
					max_response_weight: 0
				},])),
				TransferAsset { assets: (Here, SEND_AMOUNT).into(), beneficiary: sender.clone() },
			])
		);
		let status = QueryStatus::Pending {
			responder: MultiLocation::from(Parachain(PARA_ID)).into(),
			maybe_notify: None,
			timeout: 100,
		};
		assert_eq!(crate::Queries::<Test>::iter().collect::<Vec<_>>(), vec![(0, status)]);

		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm(vec![QueryResponse {
				query_id: 0,
				response: Response::ExecutionResult(None),
				max_weight: 0,
			}]),
			1_000_000_000,
		);
		assert_eq!(r, Outcome::Complete(1_000));
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::ResponseReady(
				0,
				Response::ExecutionResult(None),
			))
		);

		let response = Some((Response::ExecutionResult(None), 1));
		assert_eq!(XcmPallet::take_response(0), response);
	});
}

/// Test sending an `XCM` message (`XCM::ReserveAssetDeposit`)
///
/// Asserts that the expected message is sent and the event is emitted
#[test]
fn send_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let sender: MultiLocation =
			AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm(vec![
			ReserveAssetDeposited((Parent, SEND_AMOUNT).into()),
			ClearOrigin,
			buy_execution((Parent, SEND_AMOUNT)),
			DepositAsset { assets: All.into(), max_assets: 1, beneficiary: sender.clone() },
		]);
		let versioned_dest = Box::new(RelayLocation::get().into());
		let versioned_message = Box::new(VersionedXcm::from(message.clone()));
		assert_ok!(XcmPallet::send(
			RuntimeOrigin::signed(ALICE),
			versioned_dest,
			versioned_message
		));
		assert_eq!(
			sent_xcm(),
			vec![(
				Here.into(),
				Xcm(Some(DescendOrigin(sender.clone().try_into().unwrap()))
					.into_iter()
					.chain(message.0.clone().into_iter())
					.collect())
			)],
		);
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Sent(sender, RelayLocation::get(), message))
		);
	});
}

/// Test that sending an `XCM` message fails when the `XcmRouter` blocks the
/// matching message format
///
/// Asserts that `send` fails with `Error::SendFailure`
#[test]
fn send_fails_when_xcm_router_blocks() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let sender: MultiLocation =
			Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
		let message = Xcm(vec![
			ReserveAssetDeposited((Parent, SEND_AMOUNT).into()),
			buy_execution((Parent, SEND_AMOUNT)),
			DepositAsset { assets: All.into(), max_assets: 1, beneficiary: sender.clone() },
		]);
		assert_noop!(
			XcmPallet::send(
				RuntimeOrigin::signed(ALICE),
				Box::new(MultiLocation::ancestor(8).into()),
				Box::new(VersionedXcm::from(message.clone())),
			),
			crate::Error::<Test>::SendFailure
		);
	});
}

/// Test `teleport_assets`
///
/// Asserts that the sender's balance is decreased as a result of execution of
/// local effects.
#[test]
fn teleport_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 2 * BaseXcmWeight::get();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		let dest: MultiLocation = AccountId32 { network: Any, id: BOB.into() }.into();
		assert_ok!(XcmPallet::teleport_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(RelayLocation::get().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				RelayLocation::get().into(),
				Xcm(vec![
					ReceiveTeleportedAsset((Here, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_limited_execution((Here, SEND_AMOUNT), 4000),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		let versioned_sent = VersionedXcm::from(sent_xcm().into_iter().next().unwrap().1);
		let _check_v0_ok: xcm::v0::Xcm<()> = versioned_sent.try_into().unwrap();
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `limited_teleport_assets`
///
/// Asserts that the sender's balance is decreased as a result of execution of
/// local effects.
#[test]
fn limited_teleport_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 2 * BaseXcmWeight::get();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		let dest: MultiLocation = AccountId32 { network: Any, id: BOB.into() }.into();
		assert_ok!(XcmPallet::limited_teleport_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(RelayLocation::get().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
			WeightLimit::Limited(5000),
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				RelayLocation::get().into(),
				Xcm(vec![
					ReceiveTeleportedAsset((Here, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_limited_execution((Here, SEND_AMOUNT), 5000),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		let versioned_sent = VersionedXcm::from(sent_xcm().into_iter().next().unwrap().1);
		let _check_v0_ok: xcm::v0::Xcm<()> = versioned_sent.try_into().unwrap();
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `limited_teleport_assets` with unlimited weight
///
/// Asserts that the sender's balance is decreased as a result of execution of
/// local effects.
#[test]
fn unlimited_teleport_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 2 * BaseXcmWeight::get();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		let dest: MultiLocation = AccountId32 { network: Any, id: BOB.into() }.into();
		assert_ok!(XcmPallet::limited_teleport_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(RelayLocation::get().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
			WeightLimit::Unlimited,
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				RelayLocation::get().into(),
				Xcm(vec![
					ReceiveTeleportedAsset((Here, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_execution((Here, SEND_AMOUNT)),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `reserve_transfer_assets`
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the correct message is sent and event is emitted.
#[test]
fn reserve_transfer_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::reserve_transfer_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(Parachain(PARA_ID).into().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
		));
		// Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		// Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm(vec![
					ReserveAssetDeposited((Parent, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_limited_execution((Parent, SEND_AMOUNT), 4000),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		let versioned_sent = VersionedXcm::from(sent_xcm().into_iter().next().unwrap().1);
		let _check_v0_ok: xcm::v0::Xcm<()> = versioned_sent.try_into().unwrap();
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `limited_reserve_transfer_assets`
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the correct message is sent and event is emitted.
#[test]
fn limited_reserve_transfer_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::limited_reserve_transfer_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(Parachain(PARA_ID).into().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
			WeightLimit::Limited(5000),
		));
		// Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		// Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm(vec![
					ReserveAssetDeposited((Parent, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_limited_execution((Parent, SEND_AMOUNT), 5000),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		let versioned_sent = VersionedXcm::from(sent_xcm().into_iter().next().unwrap().1);
		let _check_v0_ok: xcm::v0::Xcm<()> = versioned_sent.try_into().unwrap();
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test `limited_reserve_transfer_assets` with unlimited weight purchasing
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the correct message is sent and event is emitted.
#[test]
fn unlimited_reserve_transfer_assets_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::limited_reserve_transfer_assets(
			RuntimeOrigin::signed(ALICE),
			Box::new(Parachain(PARA_ID).into().into()),
			Box::new(dest.clone().into()),
			Box::new((Here, SEND_AMOUNT).into()),
			0,
			WeightLimit::Unlimited,
		));
		// Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		// Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account_truncating();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + SEND_AMOUNT);
		assert_eq!(
			sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm(vec![
					ReserveAssetDeposited((Parent, SEND_AMOUNT).into()),
					ClearOrigin,
					buy_execution((Parent, SEND_AMOUNT)),
					DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
				]),
			)]
		);
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test local execution of XCM
///
/// Asserts that the sender's balance is decreased and the beneficiary's balance
/// is increased. Verifies the expected event is emitted.
#[test]
fn execute_withdraw_to_deposit_works() {
	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(ParaId::from(PARA_ID).into_account_truncating(), INITIAL_BALANCE),
	];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 3 * BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: BOB.into() }.into();
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::execute(
			RuntimeOrigin::signed(ALICE),
			Box::new(VersionedXcm::from(Xcm(vec![
				WithdrawAsset((Here, SEND_AMOUNT).into()),
				buy_execution((Here, SEND_AMOUNT)),
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
			]))),
			weight
		));
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(Balances::total_balance(&BOB), SEND_AMOUNT);
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight)))
		);
	});
}

/// Test drop/claim assets.
#[test]
fn trapped_assets_can_be_claimed() {
	let balances = vec![(ALICE, INITIAL_BALANCE), (BOB, INITIAL_BALANCE)];
	new_test_ext_with_balances(balances).execute_with(|| {
		let weight = 6 * BaseXcmWeight::get();
		let dest: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: BOB.into() }.into();

		assert_ok!(XcmPallet::execute(
			RuntimeOrigin::signed(ALICE),
			Box::new(VersionedXcm::from(Xcm(vec![
				WithdrawAsset((Here, SEND_AMOUNT).into()),
				buy_execution((Here, SEND_AMOUNT)),
				// Don't propagated the error into the result.
				SetErrorHandler(Xcm(vec![ClearError])),
				// This will make an error.
				Trap(0),
				// This would succeed, but we never get to it.
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest.clone() },
			]))),
			weight
		));
		let source: MultiLocation =
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
		let trapped = AssetTraps::<Test>::iter().collect::<Vec<_>>();
		let vma = VersionedMultiAssets::from(MultiAssets::from((Here, SEND_AMOUNT)));
		let hash = BlakeTwo256::hash_of(&(source.clone(), vma.clone()));
		assert_eq!(
			last_events(2),
			vec![
				RuntimeEvent::XcmPallet(crate::Event::AssetsTrapped(hash.clone(), source, vma)),
				RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Complete(
					5 * BaseXcmWeight::get()
				)))
			]
		);
		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(Balances::total_balance(&BOB), INITIAL_BALANCE);

		let expected = vec![(hash, 1u32)];
		assert_eq!(trapped, expected);

		let weight = 3 * BaseXcmWeight::get();
		assert_ok!(XcmPallet::execute(
			RuntimeOrigin::signed(ALICE),
			Box::new(VersionedXcm::from(Xcm(vec![
				ClaimAsset { assets: (Here, SEND_AMOUNT).into(), ticket: Here.into() },
				buy_execution((Here, SEND_AMOUNT)),
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest.clone() },
			]))),
			weight
		));

		assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - SEND_AMOUNT);
		assert_eq!(Balances::total_balance(&BOB), INITIAL_BALANCE + SEND_AMOUNT);
		assert_eq!(AssetTraps::<Test>::iter().collect::<Vec<_>>(), vec![]);

		let weight = 3 * BaseXcmWeight::get();
		assert_ok!(XcmPallet::execute(
			RuntimeOrigin::signed(ALICE),
			Box::new(VersionedXcm::from(Xcm(vec![
				ClaimAsset { assets: (Here, SEND_AMOUNT).into(), ticket: Here.into() },
				buy_execution((Here, SEND_AMOUNT)),
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: dest },
			]))),
			weight
		));
		assert_eq!(
			last_event(),
			RuntimeEvent::XcmPallet(crate::Event::Attempted(Outcome::Incomplete(
				BaseXcmWeight::get(),
				XcmError::UnknownClaim
			)))
		);
	});
}

#[test]
fn fake_latest_versioned_multilocation_works() {
	use codec::Encode;
	let remote = Parachain(1000).into();
	let versioned_remote = LatestVersionedMultiLocation(&remote);
	assert_eq!(versioned_remote.encode(), VersionedMultiLocation::from(remote.clone()).encode());
}

#[test]
fn basic_subscription_works() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote = Parachain(1000).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into()),
		));

		assert_eq!(
			Queries::<Test>::iter().collect::<Vec<_>>(),
			vec![(
				0,
				QueryStatus::VersionNotifier { origin: remote.clone().into(), is_active: false }
			)]
		);
		assert_eq!(
			VersionNotifiers::<Test>::iter().collect::<Vec<_>>(),
			vec![(2, remote.clone().into(), 0)]
		);

		assert_eq!(
			take_sent_xcm(),
			vec![(
				remote.clone(),
				Xcm(vec![SubscribeVersion { query_id: 0, max_response_weight: 0 }]),
			),]
		);

		let weight = BaseXcmWeight::get();
		let mut message = Xcm::<()>(vec![
			// Remote supports XCM v1
			QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(1) },
		]);
		assert_ok!(AllowKnownQueryResponses::<XcmPallet>::should_execute(
			&remote,
			&mut message,
			weight,
			&mut 0
		));
	});
}

#[test]
fn subscriptions_increment_id() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote = Parachain(1000).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into()),
		));

		let remote2 = Parachain(1001).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote2.clone().into()),
		));

		assert_eq!(
			take_sent_xcm(),
			vec![
				(
					remote.clone(),
					Xcm(vec![SubscribeVersion { query_id: 0, max_response_weight: 0 }]),
				),
				(
					remote2.clone(),
					Xcm(vec![SubscribeVersion { query_id: 1, max_response_weight: 0 }]),
				),
			]
		);
	});
}

#[test]
fn double_subscription_fails() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote = Parachain(1000).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into()),
		));
		assert_noop!(
			XcmPallet::force_subscribe_version_notify(
				RuntimeOrigin::root(),
				Box::new(remote.clone().into())
			),
			Error::<Test>::AlreadySubscribed,
		);
	})
}

#[test]
fn unsubscribe_works() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote = Parachain(1000).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into()),
		));
		assert_ok!(XcmPallet::force_unsubscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into())
		));
		assert_noop!(
			XcmPallet::force_unsubscribe_version_notify(
				RuntimeOrigin::root(),
				Box::new(remote.clone().into())
			),
			Error::<Test>::NoSubscription,
		);

		assert_eq!(
			take_sent_xcm(),
			vec![
				(
					remote.clone(),
					Xcm(vec![SubscribeVersion { query_id: 0, max_response_weight: 0 }]),
				),
				(remote.clone(), Xcm(vec![UnsubscribeVersion]),),
			]
		);
	});
}

/// Parachain 1000 is asking us for a version subscription.
#[test]
fn subscription_side_works() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		AdvertisedXcmVersion::set(1);

		let remote = Parachain(1000).into();
		let weight = BaseXcmWeight::get();
		let message = Xcm(vec![SubscribeVersion { query_id: 0, max_response_weight: 0 }]);
		let r = XcmExecutor::<XcmConfig>::execute_xcm(remote.clone(), message, weight);
		assert_eq!(r, Outcome::Complete(weight));

		let instr = QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(1) };
		assert_eq!(take_sent_xcm(), vec![(remote.clone(), Xcm(vec![instr]))]);

		// A runtime upgrade which doesn't alter the version sends no notifications.
		XcmPallet::on_runtime_upgrade();
		XcmPallet::on_initialize(1);
		assert_eq!(take_sent_xcm(), vec![]);

		// New version.
		AdvertisedXcmVersion::set(2);

		// A runtime upgrade which alters the version does send notifications.
		XcmPallet::on_runtime_upgrade();
		XcmPallet::on_initialize(2);
		let instr = QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(2) };
		assert_eq!(take_sent_xcm(), vec![(remote.clone(), Xcm(vec![instr]))]);
	});
}

#[test]
fn subscription_side_upgrades_work_with_notify() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		AdvertisedXcmVersion::set(1);

		// An entry from a previous runtime with v0 XCM.
		let v0_location = xcm::v0::MultiLocation::X1(xcm::v0::Junction::Parachain(1000));
		let v0_location = VersionedMultiLocation::from(v0_location);
		VersionNotifyTargets::<Test>::insert(0, v0_location, (69, 0, 1));
		let v1_location = Parachain(1001).into().versioned();
		VersionNotifyTargets::<Test>::insert(1, v1_location, (70, 0, 1));
		let v2_location = Parachain(1002).into().versioned();
		VersionNotifyTargets::<Test>::insert(2, v2_location, (71, 0, 1));

		// New version.
		AdvertisedXcmVersion::set(2);

		// A runtime upgrade which alters the version does send notifications.
		XcmPallet::on_runtime_upgrade();
		XcmPallet::on_initialize(1);

		let instr0 = QueryResponse { query_id: 69, max_weight: 0, response: Response::Version(2) };
		let instr1 = QueryResponse { query_id: 70, max_weight: 0, response: Response::Version(2) };
		let instr2 = QueryResponse { query_id: 71, max_weight: 0, response: Response::Version(2) };
		let mut sent = take_sent_xcm();
		sent.sort_by_key(|k| match (k.1).0[0] {
			QueryResponse { query_id: q, .. } => q,
			_ => 0,
		});
		assert_eq!(
			sent,
			vec![
				(Parachain(1000).into(), Xcm(vec![instr0])),
				(Parachain(1001).into(), Xcm(vec![instr1])),
				(Parachain(1002).into(), Xcm(vec![instr2])),
			]
		);

		let mut contents = VersionNotifyTargets::<Test>::iter().collect::<Vec<_>>();
		contents.sort_by_key(|k| k.2);
		assert_eq!(
			contents,
			vec![
				(2, Parachain(1000).into().versioned(), (69, 0, 2)),
				(2, Parachain(1001).into().versioned(), (70, 0, 2)),
				(2, Parachain(1002).into().versioned(), (71, 0, 2)),
			]
		);
	});
}

#[test]
fn subscription_side_upgrades_work_without_notify() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		// An entry from a previous runtime with v0 XCM.
		let v0_location = xcm::v0::MultiLocation::X1(xcm::v0::Junction::Parachain(1000));
		let v0_location = VersionedMultiLocation::from(v0_location);
		VersionNotifyTargets::<Test>::insert(0, v0_location, (69, 0, 2));
		let v1_location = Parachain(1001).into().versioned();
		VersionNotifyTargets::<Test>::insert(1, v1_location, (70, 0, 2));
		let v2_location = Parachain(1002).into().versioned();
		VersionNotifyTargets::<Test>::insert(2, v2_location, (71, 0, 2));

		// A runtime upgrade which alters the version does send notifications.
		XcmPallet::on_runtime_upgrade();
		XcmPallet::on_initialize(1);

		let mut contents = VersionNotifyTargets::<Test>::iter().collect::<Vec<_>>();
		contents.sort_by_key(|k| k.2);
		assert_eq!(
			contents,
			vec![
				(2, Parachain(1000).into().versioned(), (69, 0, 2)),
				(2, Parachain(1001).into().versioned(), (70, 0, 2)),
				(2, Parachain(1002).into().versioned(), (71, 0, 2)),
			]
		);
	});
}

#[test]
fn subscriber_side_subscription_works() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote = Parachain(1000).into();
		assert_ok!(XcmPallet::force_subscribe_version_notify(
			RuntimeOrigin::root(),
			Box::new(remote.clone().into()),
		));
		take_sent_xcm();

		// Assume subscription target is working ok.

		let weight = BaseXcmWeight::get();
		let message = Xcm(vec![
			// Remote supports XCM v1
			QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(1) },
		]);
		let r = XcmExecutor::<XcmConfig>::execute_xcm(remote.clone(), message, weight);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(take_sent_xcm(), vec![]);

		// This message cannot be sent to a v1 remote.
		let v2_msg = Xcm::<()>(vec![Trap(0)]);
		assert_eq!(XcmPallet::wrap_version(&remote, v2_msg.clone()), Err(()));

		let message = Xcm(vec![
			// Remote upgraded to XCM v2
			QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(2) },
		]);
		let r = XcmExecutor::<XcmConfig>::execute_xcm(remote.clone(), message, weight);
		assert_eq!(r, Outcome::Complete(weight));

		// This message can now be sent to remote as it's v2.
		assert_eq!(
			XcmPallet::wrap_version(&remote, v2_msg.clone()),
			Ok(VersionedXcm::from(v2_msg))
		);
	});
}

/// We should auto-subscribe when we don't know the remote's version.
#[test]
fn auto_subscription_works() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		let remote0 = Parachain(1000).into();
		let remote1 = Parachain(1001).into();

		assert_ok!(XcmPallet::force_default_xcm_version(RuntimeOrigin::root(), Some(1)));

		// Wrapping a version for a destination we don't know elicits a subscription.
		let v1_msg = xcm::v1::Xcm::<()>::QueryResponse {
			query_id: 1,
			response: xcm::v1::Response::Assets(vec![].into()),
		};
		let v2_msg = Xcm::<()>(vec![Trap(0)]);
		assert_eq!(
			XcmPallet::wrap_version(&remote0, v1_msg.clone()),
			Ok(VersionedXcm::from(v1_msg.clone())),
		);
		assert_eq!(XcmPallet::wrap_version(&remote0, v2_msg.clone()), Err(()));
		let expected = vec![(remote0.clone().into(), 2)];
		assert_eq!(VersionDiscoveryQueue::<Test>::get().into_inner(), expected);

		assert_eq!(XcmPallet::wrap_version(&remote0, v2_msg.clone()), Err(()));
		assert_eq!(XcmPallet::wrap_version(&remote1, v2_msg.clone()), Err(()));
		let expected = vec![(remote0.clone().into(), 3), (remote1.clone().into(), 1)];
		assert_eq!(VersionDiscoveryQueue::<Test>::get().into_inner(), expected);

		XcmPallet::on_initialize(1);
		assert_eq!(
			take_sent_xcm(),
			vec![(
				remote0.clone(),
				Xcm(vec![SubscribeVersion { query_id: 0, max_response_weight: 0 }]),
			)]
		);

		// Assume remote0 is working ok and XCM version 2.

		let weight = BaseXcmWeight::get();
		let message = Xcm(vec![
			// Remote supports XCM v2
			QueryResponse { query_id: 0, max_weight: 0, response: Response::Version(2) },
		]);
		let r = XcmExecutor::<XcmConfig>::execute_xcm(remote0.clone(), message, weight);
		assert_eq!(r, Outcome::Complete(weight));

		// This message can now be sent to remote0 as it's v2.
		assert_eq!(
			XcmPallet::wrap_version(&remote0, v2_msg.clone()),
			Ok(VersionedXcm::from(v2_msg.clone()))
		);

		XcmPallet::on_initialize(2);
		assert_eq!(
			take_sent_xcm(),
			vec![(
				remote1.clone(),
				Xcm(vec![SubscribeVersion { query_id: 1, max_response_weight: 0 }]),
			)]
		);

		// Assume remote1 is working ok and XCM version 1.

		let weight = BaseXcmWeight::get();
		let message = Xcm(vec![
			// Remote supports XCM v1
			QueryResponse { query_id: 1, max_weight: 0, response: Response::Version(1) },
		]);
		let r = XcmExecutor::<XcmConfig>::execute_xcm(remote1.clone(), message, weight);
		assert_eq!(r, Outcome::Complete(weight));

		// v2 messages cannot be sent to remote1...
		assert_eq!(XcmPallet::wrap_version(&remote1, v1_msg.clone()), Ok(VersionedXcm::V1(v1_msg)));
		assert_eq!(XcmPallet::wrap_version(&remote1, v2_msg.clone()), Err(()));
	})
}

#[test]
fn subscription_side_upgrades_work_with_multistage_notify() {
	new_test_ext_with_balances(vec![]).execute_with(|| {
		AdvertisedXcmVersion::set(1);

		// An entry from a previous runtime with v0 XCM.
		let v0_location = xcm::v0::MultiLocation::X1(xcm::v0::Junction::Parachain(1000));
		let v0_location = VersionedMultiLocation::from(v0_location);
		VersionNotifyTargets::<Test>::insert(0, v0_location, (69, 0, 1));
		let v1_location = Parachain(1001).into().versioned();
		VersionNotifyTargets::<Test>::insert(1, v1_location, (70, 0, 1));
		let v2_location = Parachain(1002).into().versioned();
		VersionNotifyTargets::<Test>::insert(2, v2_location, (71, 0, 1));

		// New version.
		AdvertisedXcmVersion::set(2);

		// A runtime upgrade which alters the version does send notifications.
		XcmPallet::on_runtime_upgrade();
		let mut maybe_migration = CurrentMigration::<Test>::take();
		let mut counter = 0;
		while let Some(migration) = maybe_migration.take() {
			counter += 1;
			let (_, m) = XcmPallet::check_xcm_version_change(migration, Weight::zero());
			maybe_migration = m;
		}
		assert_eq!(counter, 4);

		let instr0 = QueryResponse { query_id: 69, max_weight: 0, response: Response::Version(2) };
		let instr1 = QueryResponse { query_id: 70, max_weight: 0, response: Response::Version(2) };
		let instr2 = QueryResponse { query_id: 71, max_weight: 0, response: Response::Version(2) };
		let mut sent = take_sent_xcm();
		sent.sort_by_key(|k| match (k.1).0[0] {
			QueryResponse { query_id: q, .. } => q,
			_ => 0,
		});
		assert_eq!(
			sent,
			vec![
				(Parachain(1000).into(), Xcm(vec![instr0])),
				(Parachain(1001).into(), Xcm(vec![instr1])),
				(Parachain(1002).into(), Xcm(vec![instr2])),
			]
		);

		let mut contents = VersionNotifyTargets::<Test>::iter().collect::<Vec<_>>();
		contents.sort_by_key(|k| k.2);
		assert_eq!(
			contents,
			vec![
				(2, Parachain(1000).into().versioned(), (69, 0, 2)),
				(2, Parachain(1001).into().versioned(), (70, 0, 2)),
				(2, Parachain(1002).into().versioned(), (71, 0, 2)),
			]
		);
	});
}
