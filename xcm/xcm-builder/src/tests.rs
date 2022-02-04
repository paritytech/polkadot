// Copyright 2020 Parity Technologies query_id: (), max_response_weight: ()  query_id: (), max_response_weight: ()  (UK) Ltd.
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

use super::{mock::*, test_utils::*, *};
use frame_support::{assert_err, traits::ConstU32, weights::constants::WEIGHT_PER_SECOND};
use xcm_executor::{traits::*, Config, XcmExecutor};

#[test]
fn basic_setup_works() {
	add_reserve(Parent.into(), Wild((Parent, WildFungible).into()));
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&(Parent, 100).into(),
		&Parent.into(),
	));

	assert_eq!(to_account(Parachain(1)), Ok(1001));
	assert_eq!(to_account(Parachain(50)), Ok(1050));
	assert_eq!(to_account((Parent, Parachain(1))), Ok(2001));
	assert_eq!(to_account((Parent, Parachain(50))), Ok(2050));
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 1, network: None }))),
		Ok(1),
	);
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 42, network: None }))),
		Ok(42),
	);
	assert_eq!(to_account(Here), Ok(3000));
}

#[test]
fn weigher_should_work() {
	let mut message = Xcm(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees: (Parent, 1).into(), weight_limit: Limited(30) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Ok(30));
}

#[test]
fn take_weight_credit_barrier_should_work() {
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);
	let mut weight_credit = 10;
	let r = TakeWeightCredit::should_execute(
		&Parent.into(),
		message.inner_mut(),
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Ok(()));
	assert_eq!(weight_credit, 0);

	let r = TakeWeightCredit::should_execute(
		&Parent.into(),
		message.inner_mut(),
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Err(()));
	assert_eq!(weight_credit, 0);
}

#[test]
fn computed_origin_should_work() {
	let mut message = Xcm::<()>(vec![
		UniversalOrigin(GlobalConsensus(Kusama)),
		DescendOrigin(Parachain(100).into()),
		DescendOrigin(PalletInstance(69).into()),
		WithdrawAsset((Parent, 100).into()),
		BuyExecution { fees: (Parent, 100).into(), weight_limit: Limited(100) },
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);

	AllowPaidFrom::set(vec![(
		Parent,
		Parent,
		GlobalConsensus(Kusama),
		Parachain(100),
		PalletInstance(69),
	)
		.into()]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		100,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = WithComputedOrigin::<
		AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
		ExecutorUniversalLocation,
		ConstU32<2>,
	>::should_execute(&Parent.into(), message.inner_mut(), 100, &mut 0);
	assert_eq!(r, Err(()));

	let r = WithComputedOrigin::<
		AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
		ExecutorUniversalLocation,
		ConstU32<5>,
	>::should_execute(&Parent.into(), message.inner_mut(), 100, &mut 0);
	assert_eq!(r, Ok(()));
}

#[test]
fn allow_unpaid_should_work() {
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);

	AllowUnpaidFrom::set(vec![Parent.into()]);

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&Parachain(1).into(),
		message.inner_mut(),
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		10,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn allow_paid_should_work() {
	AllowPaidFrom::set(vec![Parent.into()]);

	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parachain(1).into(),
		message.inner_mut(),
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (Parent, 1).into();
	let mut underpaying_message = Xcm::<()>(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(20) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		underpaying_message.inner_mut(),
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (Parent, 1).into();
	let mut paying_message = Xcm::<()>(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(30) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parachain(1).into(),
		paying_message.inner_mut(),
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		paying_message.inner_mut(),
		30,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn paying_reserve_deposit_should_work() {
	AllowPaidFrom::set(vec![Parent.into()]);
	add_reserve(Parent.into(), (Parent, WildFungible).into());
	WeightPrice::set((Parent.into(), 1_000_000_000_000));

	let fees = (Parent, 30).into();
	let message = Xcm(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(30) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);
	let weight_limit = 50;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(3000), vec![(Parent, 70).into()]);
}

#[test]
fn transfer_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000));
	// They want to transfer 100 of them to their sibling parachain #2
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![TransferAsset {
			assets: (Here, 100).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(assets(3), vec![(Here, 100).into()]);
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn universal_origin_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into(), X1(Parachain(2)).into()]);
	clear_universal_aliases();
	// Parachain 1 may represent Kusama to us
	add_universal_alias(Parachain(1), Kusama);
	// Parachain 2 may represent Polkadot to us
	add_universal_alias(Parachain(2), Polkadot);

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(2),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::InvalidLocation));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::NotWithdrawable));

	add_asset(4000, (Parent, 100));
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(20));
	assert_eq!(assets(4000), vec![]);
}

#[test]
fn export_message_should_work() {
	// Bridge chain (assumed to be Relay) lets Parachain #1 have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Local parachain #1 issues a transfer asset on Polkadot Relay-chain, transfering 100 Planck to
	// Polkadot parachain #2.
	let message =
		Xcm(vec![TransferAsset { assets: (Here, 100).into(), beneficiary: Parachain(2).into() }]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExportMessage { network: Polkadot, destination: Here, xcm: message.clone() }]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(exported_xcm(), vec![(Polkadot, 403611790, Here, message)]);
}

#[test]
fn basic_asset_trap_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into(), X1(Parachain(2)).into()]);

	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000));
	// They want to transfer 100 of them to their sibling parachain #2 but have a problem
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 100).into()),
			DepositAsset {
				assets: Wild(AllCounted(0)), // <<< 0 is an error.
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Complete(25));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(assets(3), vec![]);

	// Incorrect ticket doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100).into(), ticket: GeneralIndex(1).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(assets(3), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	// Incorrect origin doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(2),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100).into(), ticket: GeneralIndex(0).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(assets(3), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	// Incorrect assets doesn't work.
	let old_trapped_assets = TrappedAssets::get();
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 101).into(), ticket: GeneralIndex(0).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::UnknownClaim));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(assets(3), vec![]);
	assert_eq!(old_trapped_assets, TrappedAssets::get());

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100).into(), ticket: GeneralIndex(0).into() },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: AccountIndex64 { index: 3, network: None }.into(),
			},
		]),
		20,
	);
	assert_eq!(r, Outcome::Complete(20));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(assets(3), vec![(Here, 100).into()]);

	// Same again doesn't work :-)
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			ClaimAsset { assets: (Here, 100).into(), ticket: GeneralIndex(0).into() },
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
fn errors_should_return_unused_weight() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![Here.into()]);
	// We own 1000 of our tokens.
	add_asset(3000, (Here, 11));
	let mut message = Xcm(vec![
		// First xfer results in an error on the last message only
		TransferAsset {
			assets: (Here, 1).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Second xfer results in error third message and after
		TransferAsset {
			assets: (Here, 2).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Third xfer results in error second message and after
		TransferAsset {
			assets: (Here, 4).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
	]);
	// Weight limit of 70 is needed.
	let limit = <TestConfig as Config>::Weigher::weight(&mut message).unwrap();
	assert_eq!(limit, 30);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(3), vec![(Here, 7).into()]);
	assert_eq!(assets(3000), vec![(Here, 4).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Incomplete(30, XcmError::NotWithdrawable));
	assert_eq!(assets(3), vec![(Here, 10).into()]);
	assert_eq!(assets(3000), vec![(Here, 1).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::NotWithdrawable));
	assert_eq!(assets(3), vec![(Here, 11).into()]);
	assert_eq!(assets(3000), vec![]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message, limit);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NotWithdrawable));
	assert_eq!(assets(3), vec![(Here, 11).into()]);
	assert_eq!(assets(3000), vec![]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn weight_bounds_should_respect_instructions_limit() {
	MaxInstructions::set(3);
	let mut message = Xcm(vec![ClearOrigin; 4]);
	// 4 instructions are too many.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![ClearOrigin])), SetAppendix(Xcm(vec![ClearOrigin]))]);
	// 4 instructions are too many, even when hidden within 2.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(
			vec![ClearOrigin],
		))]))]))]);
	// 4 instructions are too many, even when it's just one that's 3 levels deep.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(vec![ClearOrigin]))]))]);
	// 3 instructions are OK.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Ok(30));
}

#[test]
fn code_registers_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![Here.into()]);
	// We own 1000 of our tokens.
	add_asset(3000, (Here, 21));
	let mut message = Xcm(vec![
		// Set our error handler - this will fire only on the second message, when there's an error
		SetErrorHandler(Xcm(vec![
			TransferAsset {
				assets: (Here, 2).into(),
				beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
			},
			// It was handled fine.
			ClearError,
		])),
		// Set the appendix - this will always fire.
		SetAppendix(Xcm(vec![TransferAsset {
			assets: (Here, 4).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		}])),
		// First xfer always works ok
		TransferAsset {
			assets: (Here, 1).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Second xfer results in error on the second message - our error handler will fire.
		TransferAsset {
			assets: (Here, 8).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
	]);
	// Weight limit of 70 is needed.
	let limit = <TestConfig as Config>::Weigher::weight(&mut message).unwrap();
	assert_eq!(limit, 70);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Complete(50)); // We don't pay the 20 weight for the error handler.
	assert_eq!(assets(3), vec![(Here, 13).into()]);
	assert_eq!(assets(3000), vec![(Here, 8).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message, limit);
	assert_eq!(r, Outcome::Complete(70)); // We pay the full weight here.
	assert_eq!(assets(3), vec![(Here, 20).into()]);
	assert_eq!(assets(3000), vec![(Here, 1).into()]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn reserve_transfer_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000));
	// The remote account owned by gav.
	let three: MultiLocation = X1(AccountIndex64 { index: 3, network: None }).into();

	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![TransferReserveAsset {
			assets: (Here, 100).into(),
			dest: Parachain(2).into(),
			xcm: Xcm::<()>(vec![DepositAsset {
				assets: AllCounted(1).into(),
				beneficiary: three.clone(),
			}]),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(assets(1002), vec![(Here, 100).into()]);
	assert_eq!(
		sent_xcm(),
		vec![(
			Parachain(2).into(),
			Xcm::<()>(vec![
				ReserveAssetDeposited((Parent, 100).into()),
				ClearOrigin,
				DepositAsset { assets: AllCounted(1).into(), beneficiary: three },
			]),
		)]
	);
}

#[test]
fn simple_version_subscriptions_should_work() {
	AllowSubsFrom::set(vec![Parent.into()]);

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![
		SetAppendix(Xcm(vec![])),
		SubscribeVersion { query_id: 42, max_response_weight: 5000 },
	]);
	let weight_limit = 20;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let origin = Parachain(1000);
	let message =
		Xcm::<TestCall>(vec![SubscribeVersion { query_id: 42, max_response_weight: 5000 }]);
	let weight_limit = 10;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message.clone(), weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(SubscriptionRequests::get(), vec![(Parent.into(), Some((42, 5000)))]);
}

#[test]
fn version_subscription_instruction_should_work() {
	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![
		DescendOrigin(X1(AccountIndex64 { index: 1, network: None })),
		SubscribeVersion { query_id: 42, max_response_weight: 5000 },
	]);
	let weight_limit = 20;
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin.clone(),
		message.clone(),
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::BadOrigin));

	let message = Xcm::<TestCall>(vec![
		SetAppendix(Xcm(vec![])),
		SubscribeVersion { query_id: 42, max_response_weight: 5000 },
	]);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message.clone(),
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Complete(20));

	assert_eq!(SubscriptionRequests::get(), vec![(Parachain(1000).into(), Some((42, 5000)))]);
}

#[test]
fn simple_version_unsubscriptions_should_work() {
	AllowSubsFrom::set(vec![Parent.into()]);

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![SetAppendix(Xcm(vec![])), UnsubscribeVersion]);
	let weight_limit = 20;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![UnsubscribeVersion]);
	let weight_limit = 10;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message.clone(), weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(SubscriptionRequests::get(), vec![(Parent.into(), None)]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn version_unsubscription_instruction_should_work() {
	let origin = Parachain(1000);

	// Not allowed to do it when origin has been changed.
	let message = Xcm::<TestCall>(vec![
		DescendOrigin(X1(AccountIndex64 { index: 1, network: None })),
		UnsubscribeVersion,
	]);
	let weight_limit = 20;
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin.clone(),
		message.clone(),
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::BadOrigin));

	// Fine to do it when origin is untouched.
	let message = Xcm::<TestCall>(vec![SetAppendix(Xcm(vec![])), UnsubscribeVersion]);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message.clone(),
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Complete(20));

	assert_eq!(SubscriptionRequests::get(), vec![(Parachain(1000).into(), None)]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, None).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(60));
}

#[test]
fn transacting_should_respect_max_weight_requirement() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: 40,
		call: TestCall::Any(50, None).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Incomplete(50, XcmError::MaxWeightInvalid));
}

#[test]
fn transacting_should_refund_weight() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, Some(30)).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(40));
}

#[test]
fn paid_transacting_should_refund_payment_for_unused_weight() {
	let one: MultiLocation = AccountIndex64 { index: 1, network: None }.into();
	AllowPaidFrom::set(vec![one.clone()]);
	add_asset(1, (Parent, 100));
	WeightPrice::set((Parent.into(), 1_000_000_000_000));

	let origin = one.clone();
	let fees = (Parent, 100).into();
	let message = Xcm::<TestCall>(vec![
		WithdrawAsset((Parent, 100).into()), // enough for 100 units of weight.
		BuyExecution { fees, weight_limit: Limited(100) },
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: 50,
			// call estimated at 50 but only takes 10.
			call: TestCall::Any(50, Some(10)).encode().into(),
		},
		RefundSurplus,
		DepositAsset { assets: AllCounted(1).into(), beneficiary: one.clone() },
	]);
	let weight_limit = 100;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(60));
	assert_eq!(assets(1), vec![(Parent, 40).into()]);
}

#[test]
fn prepaid_result_of_query_should_get_free_execution() {
	let query_id = 33;
	// We put this in manually here, but normally this would be done at the point of crafting the message.
	expect_response(query_id, Parent.into());

	let the_response = Response::Assets((Parent, 100).into());
	let message = Xcm::<TestCall>(vec![QueryResponse {
		query_id,
		response: the_response.clone(),
		max_weight: 10,
		querier: Some(Here.into()),
	}]);
	let weight_limit = 10;

	// First time the response gets through since we're expecting it...
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message.clone(), weight_limit);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(response(query_id).unwrap(), the_response);

	// Second time it doesn't, since we're not.
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message.clone(), weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));
}

fn fungible_multi_asset(location: MultiLocation, amount: u128) -> MultiAsset {
	(AssetId::from(location), Fungibility::Fungible(amount)).into()
}

#[test]
fn weight_trader_tuple_should_work() {
	let para_1: MultiLocation = Parachain(1).into();
	let para_2: MultiLocation = Parachain(2).into();

	parameter_types! {
		pub static HereWeightPrice: (AssetId, u128) = (Here.into(), WEIGHT_PER_SECOND.into());
		pub static Para1WeightPrice: (AssetId, u128) = (Parachain(1).into(), WEIGHT_PER_SECOND.into());
	}

	type Traders = (
		// trader one
		FixedRateOfFungible<HereWeightPrice, ()>,
		// trader two
		FixedRateOfFungible<Para1WeightPrice, ()>,
	);

	let mut traders = Traders::new();
	// trader one buys weight
	assert_eq!(
		traders.buy_weight(5, fungible_multi_asset(Here.into(), 10).into()),
		Ok(fungible_multi_asset(Here.into(), 5).into()),
	);
	// trader one refunds
	assert_eq!(traders.refund_weight(2), Some(fungible_multi_asset(Here.into(), 2)));

	let mut traders = Traders::new();
	// trader one failed; trader two buys weight
	assert_eq!(
		traders.buy_weight(5, fungible_multi_asset(para_1.clone(), 10).into()),
		Ok(fungible_multi_asset(para_1.clone(), 5).into()),
	);
	// trader two refunds
	assert_eq!(traders.refund_weight(2), Some(fungible_multi_asset(para_1, 2)));

	let mut traders = Traders::new();
	// all traders fails
	assert_err!(
		traders.buy_weight(5, fungible_multi_asset(para_2, 10).into()),
		XcmError::TooExpensive,
	);
	// and no refund
	assert_eq!(traders.refund_weight(2), None);
}

#[test]
fn pallet_query_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![QueryPallet {
			module_name: "Error".into(),
			response_info: QueryResponseInfo {
				destination: Parachain(1).into(),
				query_id: 1,
				max_weight: 50,
			},
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(
		sent_xcm(),
		vec![(
			Parachain(1).into(),
			Xcm::<()>(vec![QueryResponse {
				query_id: 1,
				max_weight: 50,
				response: Response::PalletsInfo(vec![]),
				querier: Some(Here.into()),
			}]),
		)]
	);
}

#[test]
fn pallet_query_with_results_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![QueryPallet {
			module_name: "pallet_balances".into(),
			response_info: QueryResponseInfo {
				destination: Parachain(1).into(),
				query_id: 1,
				max_weight: 50,
			},
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(
		sent_xcm(),
		vec![(
			Parachain(1).into(),
			Xcm::<()>(vec![QueryResponse {
				query_id: 1,
				max_weight: 50,
				response: Response::PalletsInfo(vec![PalletInfo {
					index: 1,
					name: b"Balances".as_ref().into(),
					module_name: b"pallet_balances".as_ref().into(),
					major: 1,
					minor: 42,
					patch: 69,
				},]),
				querier: Some(Here.into()),
			}]),
		)]
	);
}

#[test]
fn report_successful_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: 50,
			call: TestCall::Any(50, None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: 5000,
		}),
	]);
	let weight_limit = 70;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(70));
	assert_eq!(
		sent_xcm(),
		vec![(
			Parent.into(),
			Xcm(vec![QueryResponse {
				response: Response::DispatchResult(MaybeErrorCode::Success),
				query_id: 42,
				max_weight: 5000,
				querier: Some(Here.into()),
			}])
		)]
	);
}

#[test]
fn report_failed_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: 50,
			call: TestCall::OnlyRoot(50, None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: 5000,
		}),
	]);
	let weight_limit = 70;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(70));
	assert_eq!(
		sent_xcm(),
		vec![(
			Parent.into(),
			Xcm(vec![QueryResponse {
				response: Response::DispatchResult(MaybeErrorCode::Error(vec![2])),
				query_id: 42,
				max_weight: 5000,
				querier: Some(Here.into()),
			}])
		)]
	);
}

#[test]
fn clear_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: 50,
			call: TestCall::OnlyRoot(50, None).encode().into(),
		},
		ClearTransactStatus,
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: 5000,
		}),
	]);
	let weight_limit = 80;
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, weight_limit);
	assert_eq!(r, Outcome::Complete(80));
	assert_eq!(
		sent_xcm(),
		vec![(
			Parent.into(),
			Xcm(vec![QueryResponse {
				response: Response::DispatchResult(MaybeErrorCode::Success),
				query_id: 42,
				max_weight: 5000,
				querier: Some(Here.into()),
			}])
		)]
	);
}

#[test]
fn max_assets_limit_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (vec![1], 1000));
	add_asset(1001, (vec![2], 1000));
	add_asset(1001, (vec![3], 1000));
	add_asset(1001, (vec![4], 1000));
	add_asset(1001, (vec![5], 1000));
	add_asset(1001, (vec![6], 1000));
	add_asset(1001, (vec![7], 1000));
	add_asset(1001, (vec![8], 1000));
	add_asset(1001, (vec![9], 1000));

	// Attempt to withdraw 8 (=2x4)different assets. This will succeed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
			WithdrawAsset((vec![5], 100).into()),
			WithdrawAsset((vec![6], 100).into()),
			WithdrawAsset((vec![7], 100).into()),
			WithdrawAsset((vec![8], 100).into()),
		]),
		100,
	);
	assert_eq!(r, Outcome::Complete(85));

	// Attempt to withdraw 9 different assets will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
			WithdrawAsset((vec![5], 100).into()),
			WithdrawAsset((vec![6], 100).into()),
			WithdrawAsset((vec![7], 100).into()),
			WithdrawAsset((vec![8], 100).into()),
			WithdrawAsset((vec![9], 100).into()),
		]),
		100,
	);
	assert_eq!(r, Outcome::Incomplete(95, XcmError::HoldingWouldOverflow));

	// Attempt to withdraw 4 different assets and then the same 4 and then a different 4 will succeed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
			WithdrawAsset((vec![5], 100).into()),
			WithdrawAsset((vec![6], 100).into()),
			WithdrawAsset((vec![7], 100).into()),
			WithdrawAsset((vec![8], 100).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Complete(125));

	// Attempt to withdraw 4 different assets and then a different 4 and then the same 4 will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
			WithdrawAsset((vec![5], 100).into()),
			WithdrawAsset((vec![6], 100).into()),
			WithdrawAsset((vec![7], 100).into()),
			WithdrawAsset((vec![8], 100).into()),
			WithdrawAsset((vec![1], 100).into()),
			WithdrawAsset((vec![2], 100).into()),
			WithdrawAsset((vec![3], 100).into()),
			WithdrawAsset((vec![4], 100).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Incomplete(95, XcmError::HoldingWouldOverflow));

	// Attempt to withdraw 4 different assets and then a different 4 and then the same 4 will fail.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset(MultiAssets::from(vec![
				(vec![1], 100).into(),
				(vec![2], 100).into(),
				(vec![3], 100).into(),
				(vec![4], 100).into(),
				(vec![5], 100).into(),
				(vec![6], 100).into(),
				(vec![7], 100).into(),
				(vec![8], 100).into(),
			])),
			WithdrawAsset((vec![1], 100).into()),
		]),
		200,
	);
	assert_eq!(r, Outcome::Incomplete(25, XcmError::HoldingWouldOverflow));
}

#[test]
fn burn_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000));
	// They want to burn 100 of them
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 1000).into()),
			BurnAsset((Here, 100).into()),
			DepositAsset { assets: Wild(AllCounted(1)), beneficiary: Parachain(1).into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(sent_xcm(), vec![]);

	// Now they want to burn 1000 of them, which will actually only burn 900.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			WithdrawAsset((Here, 900).into()),
			BurnAsset((Here, 1000).into()),
			DepositAsset { assets: Wild(AllCounted(1)), beneficiary: Parachain(1).into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(1001), vec![]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn expect_pallet_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 41,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
}

#[test]
fn expect_pallet_should_fail_correctly() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 60,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::VersionIncompatible));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"System".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NameMismatch));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_system".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NameMismatch));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 0,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NameMismatch));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 2,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::PalletNotFound));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 2,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::VersionIncompatible));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 0,
			min_crate_minor: 42,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::VersionIncompatible));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExpectPallet {
			index: 1,
			name: b"Balances".as_ref().into(),
			module_name: b"pallet_balances".as_ref().into(),
			crate_major: 1,
			min_crate_minor: 43,
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::VersionIncompatible));
}
