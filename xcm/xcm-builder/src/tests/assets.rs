// Copyright 2022 Parity Technologies (UK) Ltd.
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

use super::*;

#[test]
fn exchange_asset_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);
	add_asset(Parent, (Parent, 1000u128));
	set_exchange_assets(vec![(Here, 100u128).into()]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parent,
		Xcm(vec![
			WithdrawAsset((Parent, 100u128).into()),
			SetAppendix(
				vec![DepositAsset { assets: AllCounted(2).into(), beneficiary: Parent.into() }]
					.into(),
			),
			ExchangeAsset {
				give: Definite((Parent, 50u128).into()),
				want: (Here, 50u128).into(),
				maximal: true,
			},
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(40));
	assert_eq!(asset_list(Parent), vec![(Here, 100u128).into(), (Parent, 950u128).into()]);
	assert_eq!(exchange_assets(), vec![(Parent, 50u128).into()].into());
}

#[test]
fn exchange_asset_without_maximal_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);
	add_asset(Parent, (Parent, 1000));
	set_exchange_assets(vec![(Here, 100u128).into()]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parent,
		Xcm(vec![
			WithdrawAsset((Parent, 100u128).into()),
			SetAppendix(
				vec![DepositAsset { assets: AllCounted(2).into(), beneficiary: Parent.into() }]
					.into(),
			),
			ExchangeAsset {
				give: Definite((Parent, 50u128).into()),
				want: (Here, 50u128).into(),
				maximal: false,
			},
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(40));
	assert_eq!(asset_list(Parent), vec![(Here, 50u128).into(), (Parent, 950u128).into()]);
	assert_eq!(exchange_assets(), vec![(Here, 50u128).into(), (Parent, 50u128).into()].into());
}

#[test]
fn exchange_asset_should_fail_when_no_deal_possible() {
	AllowUnpaidFrom::set(vec![Parent.into()]);
	add_asset(Parent, (Parent, 1000));
	set_exchange_assets(vec![(Here, 100u128).into()]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parent,
		Xcm(vec![
			WithdrawAsset((Parent, 150u128).into()),
			SetAppendix(
				vec![DepositAsset { assets: AllCounted(2).into(), beneficiary: Parent.into() }]
					.into(),
			),
			ExchangeAsset {
				give: Definite((Parent, 150u128).into()),
				want: (Here, 150u128).into(),
				maximal: false,
			},
		]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(40, XcmError::NoDeal));
	assert_eq!(asset_list(Parent), vec![(Parent, 1000u128).into()]);
	assert_eq!(exchange_assets(), vec![(Here, 100u128).into()].into());
}

#[test]
fn paying_reserve_deposit_should_work() {
	AllowPaidFrom::set(vec![Parent.into()]);
	add_reserve(Parent.into(), (Parent, WildFungible).into());
	WeightPrice::set((Parent.into(), 1_000_000_000_000));

	let fees = (Parent, 30u128).into();
	let message = Xcm(vec![
		ReserveAssetDeposited((Parent, 100u128).into()),
		BuyExecution { fees, weight_limit: Limited(30) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);
	let weight_limit = 50;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(asset_list(Here), vec![(Parent, 70u128).into()]);
}

#[test]
fn transfer_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(Parachain(1), (Here, 1000));
	// They want to transfer 100 of them to their sibling parachain #2
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![TransferAsset {
			assets: (Here, 100u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 100u128).into()]);
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn reserve_transfer_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(Parachain(1), (Here, 1000));
	// The remote account owned by gav.
	let three: MultiLocation = X1(AccountIndex64 { index: 3, network: None }).into();

	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![TransferReserveAsset {
			assets: (Here, 100u128).into(),
			dest: Parachain(2).into(),
			xcm: Xcm::<()>(vec![DepositAsset {
				assets: AllCounted(1).into(),
				beneficiary: three.clone(),
			}]),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(asset_list(Parachain(2)), vec![(Here, 100u128).into()]);
	assert_eq!(
		sent_xcm(),
		vec![(
			Parachain(2).into(),
			Xcm::<()>(vec![
				ReserveAssetDeposited((Parent, 100u128).into()),
				ClearOrigin,
				DepositAsset { assets: AllCounted(1).into(), beneficiary: three },
			]),
		)]
	);
}

#[test]
fn burn_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(Parachain(1), (Here, 1000));
	// They want to burn 100 of them
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 1000u128).into()),
			BurnAsset((Here, 100u128).into()),
			DepositAsset { assets: Wild(AllCounted(1)), beneficiary: Parachain(1).into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(sent_xcm(), vec![]);

	// Now they want to burn 1000 of them, which will actually only burn 900.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 900u128).into()),
			BurnAsset((Here, 1000u128).into()),
			DepositAsset { assets: Wild(AllCounted(1)), beneficiary: Parachain(1).into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(asset_list(Parachain(1)), vec![]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn basic_asset_trap_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into(), X1(Parachain(2)).into()]);

	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(Parachain(1), (Here, 1000));
	// They want to transfer 100 of them to their sibling parachain #2 but have a problem
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 100u128).into()),
			DepositAsset {
				assets: Wild(AllCounted(0)), // <<< 0 is an error.
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Complete(25));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![]);

	// Incorrect ticket doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100u128).into(), ticket: GeneralIndex(1).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	// Incorrect origin doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(2),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100u128).into(), ticket: GeneralIndex(0u128).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	// Incorrect assets doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 101u128).into(), ticket: GeneralIndex(0u128).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100u128).into(), ticket: GeneralIndex(0u128).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Complete(20));
	assert_eq!(asset_list(Parachain(1)), vec![(Here, 900u128).into()]);
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 100u128).into()]);

	// Same again doesn't work :-)
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100u128).into(), ticket: GeneralIndex(0u128).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
}

#[test]
fn max_assets_limit_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(Parachain(1), ([1u8; 32], 1000u128));
	add_asset(Parachain(1), ([2u8; 32], 1000u128));
	add_asset(Parachain(1), ([3u8; 32], 1000u128));
	add_asset(Parachain(1), ([4u8; 32], 1000u128));
	add_asset(Parachain(1), ([5u8; 32], 1000u128));
	add_asset(Parachain(1), ([6u8; 32], 1000u128));
	add_asset(Parachain(1), ([7u8; 32], 1000u128));
	add_asset(Parachain(1), ([8u8; 32], 1000u128));
	add_asset(Parachain(1), ([9u8; 32], 1000u128));

	// Attempt to withdraw 8 (=2x4)different assets. This will succeed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
			WithdrawAsset(([5u8; 32], 100u128).into()),
			WithdrawAsset(([6u8; 32], 100u128).into()),
			WithdrawAsset(([7u8; 32], 100u128).into()),
			WithdrawAsset(([8u8; 32], 100u128).into()),
		]),
		100,
	);
	assert_eq!(r, Outcome::Complete(85));

	// Attempt to withdraw 9 different assets will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
			WithdrawAsset(([5u8; 32], 100u128).into()),
			WithdrawAsset(([6u8; 32], 100u128).into()),
			WithdrawAsset(([7u8; 32], 100u128).into()),
			WithdrawAsset(([8u8; 32], 100u128).into()),
			WithdrawAsset(([9u8; 32], 100u128).into()),
		]),
		100,
	);
	assert_eq!(r, Outcome::Incomplete(95, XcmError::HoldingWouldOverflow));

	// Attempt to withdraw 4 different assets and then the same 4 and then a different 4 will succeed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
			WithdrawAsset(([5u8; 32], 100u128).into()),
			WithdrawAsset(([6u8; 32], 100u128).into()),
			WithdrawAsset(([7u8; 32], 100u128).into()),
			WithdrawAsset(([8u8; 32], 100u128).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Complete(125));

	// Attempt to withdraw 4 different assets and then a different 4 and then the same 4 will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
			WithdrawAsset(([5u8; 32], 100u128).into()),
			WithdrawAsset(([6u8; 32], 100u128).into()),
			WithdrawAsset(([7u8; 32], 100u128).into()),
			WithdrawAsset(([8u8; 32], 100u128).into()),
			WithdrawAsset(([1u8; 32], 100u128).into()),
			WithdrawAsset(([2u8; 32], 100u128).into()),
			WithdrawAsset(([3u8; 32], 100u128).into()),
			WithdrawAsset(([4u8; 32], 100u128).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Incomplete(95, XcmError::HoldingWouldOverflow));

	// Attempt to withdraw 4 different assets and then a different 4 and then the same 4 will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(MultiAssets::from(vec![
				([1u8; 32], 100u128).into(),
				([2u8; 32], 100u128).into(),
				([3u8; 32], 100u128).into(),
				([4u8; 32], 100u128).into(),
				([5u8; 32], 100u128).into(),
				([6u8; 32], 100u128).into(),
				([7u8; 32], 100u128).into(),
				([8u8; 32], 100u128).into(),
			])),
			WithdrawAsset(([1u8; 32], 100u128).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Incomplete(25, XcmError::HoldingWouldOverflow));
}
