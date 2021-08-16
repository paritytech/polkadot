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

use super::{mock::*, *};
use xcm::latest::prelude::*;
use xcm_executor::{traits::*, Config, XcmExecutor};

#[test]
fn basic_setup_works() {
	add_reserve(Parent.into(), Wild((Parent, WildFungible).into()));
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&(Parent, 100).into(),
		&Parent.into(),
	));

	assert_eq!(to_account(X1(Parachain(1)).into()), Ok(1001));
	assert_eq!(to_account(X1(Parachain(50)).into()), Ok(1050));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(1)))), Ok(2001));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(50)))), Ok(2050));
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 1, network: Any }))),
		Ok(1),
	);
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 42, network: Any }))),
		Ok(42),
	);
	assert_eq!(to_account(Here.into()), Ok(3000));
}

#[test]
fn weigher_should_work() {
	let mut message = Xcm(vec![
		ReserveAssetDeposited { assets: (Parent, 100).into() },
		BuyExecution { fees: (Parent, 1).into(), weight_limit: Limited(30) },
		DepositAsset { assets: All.into(), max_assets: 1, beneficiary: Here.into() },
	]);
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Ok(30));
}

#[test]
fn take_weight_credit_barrier_should_work() {
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);
	let mut weight_credit = 10;
	let r = TakeWeightCredit::should_execute(
		&Some(Parent.into()),
		true,
		&mut message,
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Ok(()));
	assert_eq!(weight_credit, 0);

	let r = TakeWeightCredit::should_execute(
		&Some(Parent.into()),
		true,
		&mut message,
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Err(()));
	assert_eq!(weight_credit, 0);
}

#[test]
fn allow_unpaid_should_work() {
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);

	AllowUnpaidFrom::set(vec![Parent.into()]);

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&Some(Parachain(1).into()),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&Some(Parent.into()),
		true,
		&mut message,
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
		&Some(Parachain(1).into()),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (Parent, 1).into();
	let mut underpaying_message = Xcm::<()>(vec![
		ReserveAssetDeposited { assets: (Parent, 100).into() },
		BuyExecution { fees, weight_limit: Limited(20) },
		DepositAsset { assets: All.into(), max_assets: 1, beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Some(Parent.into()),
		true,
		&mut underpaying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (Parent, 1).into();
	let mut paying_message = Xcm::<()>(vec![
		ReserveAssetDeposited { assets: (Parent, 100).into() },
		BuyExecution { fees, weight_limit: Limited(30) },
		DepositAsset { assets: All.into(), max_assets: 1, beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Some(Parachain(1).into()),
		true,
		&mut paying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Some(Parent.into()),
		true,
		&mut paying_message,
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

	let origin = Parent.into();
	let fees = (Parent, 30).into();
	let message = Xcm(vec![
		ReserveAssetDeposited { assets: (Parent, 100).into() },
		BuyExecution { fees, weight_limit: Limited(30) },
		DepositAsset { assets: All.into(), max_assets: 1, beneficiary: Here.into() },
	]);
	let weight_limit = 50;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(3000), vec![(Parent, 70).into()]);
}

#[test]
fn transfer_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000).into());
	// They want to transfer 100 of them to their sibling parachain #2
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1).into(),
		Xcm(vec![TransferAsset {
			assets: (Here, 100).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: Any }).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(assets(3), vec![(Here, 100).into()]);
	assert_eq!(assets(1001), vec![(Here, 900).into()]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn reserve_transfer_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here, 1000).into());
	// The remote account owned by gav.
	let three: MultiLocation = X1(AccountIndex64 { index: 3, network: Any }).into();

	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1).into(),
		Xcm(vec![TransferReserveAsset {
			assets: (Here, 100).into(),
			dest: Parachain(2).into(),
			xcm: Xcm::<()>(vec![DepositAsset {
				assets: All.into(),
				max_assets: 1,
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
				ReserveAssetDeposited { assets: (Parent, 100).into() },
				ClearOrigin,
				DepositAsset { assets: All.into(), max_assets: 1, beneficiary: three },
			]),
		)]
	);
}

#[test]
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let origin = Parent.into();
	let message = Xcm::<TestCall>(vec![Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, None).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(60));
}

#[test]
fn transacting_should_respect_max_weight_requirement() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let origin = Parent.into();
	let message = Xcm::<TestCall>(vec![Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 40,
		call: TestCall::Any(50, None).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Incomplete(50, XcmError::TooMuchWeightRequired));
}

#[test]
fn transacting_should_refund_weight() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let origin = Parent.into();
	let message = Xcm::<TestCall>(vec![Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, Some(30)).encode().into(),
	}]);
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(40));
}

#[test]
fn paid_transacting_should_refund_payment_for_unused_weight() {
	let one: MultiLocation = X1(AccountIndex64 { index: 1, network: Any }).into();
	AllowPaidFrom::set(vec![one.clone()]);
	add_asset(1, (Parent, 100).into());
	WeightPrice::set((Parent.into(), 1_000_000_000_000));

	let origin = one.clone();
	let fees = (Parent, 100).into();
	let message = Xcm::<TestCall>(vec![
		WithdrawAsset { assets: (Parent, 100).into() }, // enough for 100 units of weight.
		BuyExecution { fees, weight_limit: Limited(100) },
		Transact {
			origin_type: OriginKind::Native,
			require_weight_at_most: 50,
			// call estimated at 70 but only takes 10.
			call: TestCall::Any(50, Some(10)).encode().into(),
		},
		RefundSurplus,
		DepositAsset { assets: All.into(), max_assets: 1, beneficiary: one.clone() },
	]);
	let weight_limit = 100;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(60));
	assert_eq!(assets(1), vec![(Parent, 40).into()]);
}

#[test]
fn prepaid_result_of_query_should_get_free_execution() {
	let query_id = 33;
	let origin: MultiLocation = Parent.into();
	// We put this in manually here, but normally this would be done at the point of crafting the message.
	expect_response(query_id, origin.clone());

	let the_response = Response::Assets((Parent, 100).into());
	let message = Xcm::<TestCall>(vec![QueryResponse {
		query_id,
		response: the_response.clone(),
		max_weight: 10,
	}]);
	let weight_limit = 10;

	// First time the response gets through since we're expecting it...
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(response(query_id).unwrap(), the_response);

	// Second time it doesn't, since we're not.
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::Barrier));
}
