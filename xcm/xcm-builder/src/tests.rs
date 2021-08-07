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
	add_reserve(
		MultiLocation::with_parents::<1>(),
		Wild((MultiLocation::with_parents::<1>(), WildFungible).into()),
	);
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&(MultiLocation::with_parents::<1>(), 100).into(),
		&MultiLocation::with_parents::<1>(),
	));

	assert_eq!(to_account(X1(Parachain(1)).into()), Ok(1001));
	assert_eq!(to_account(X1(Parachain(50)).into()), Ok(1050));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(1))).unwrap()), Ok(2001));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(50))).unwrap()), Ok(2050));
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 1, network: Any })).unwrap()),
		Ok(1),
	);
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 42, network: Any })).unwrap()),
		Ok(42),
	);
	assert_eq!(to_account(Here.into()), Ok(3000));
}

#[test]
fn weigher_should_work() {
	let mut message = opaque::Xcm::ReserveAssetDeposited {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		effects: vec![
			Order::BuyExecution {
				fees: (MultiLocation::with_parents::<1>(), 1).into(),
				weight: 0,
				debt: 30,
				halt_on_error: true,
				orders: vec![],
				instructions: vec![],
			},
			Order::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		],
	}
	.into();
	assert_eq!(<TestConfig as Config>::Weigher::shallow(&mut message), Ok(30));
}

#[test]
fn take_weight_credit_barrier_should_work() {
	let mut message = opaque::Xcm::TransferAsset {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		beneficiary: Here.into(),
	};

	let mut weight_credit = 10;
	let r = TakeWeightCredit::should_execute(
		&MultiLocation::with_parents::<1>(),
		true,
		&mut message,
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Ok(()));
	assert_eq!(weight_credit, 0);

	let r = TakeWeightCredit::should_execute(
		&MultiLocation::with_parents::<1>(),
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
	let mut message = opaque::Xcm::TransferAsset {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		beneficiary: Here.into(),
	};

	AllowUnpaidFrom::set(vec![MultiLocation::with_parents::<1>()]);

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&X1(Parachain(1)).into(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&MultiLocation::with_parents::<1>(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn allow_paid_should_work() {
	AllowPaidFrom::set(vec![MultiLocation::with_parents::<1>()]);

	let mut message = opaque::Xcm::TransferAsset {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		beneficiary: Here.into(),
	};

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&X1(Parachain(1)).into(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (MultiLocation::with_parents::<1>(), 1).into();
	let mut underpaying_message = opaque::Xcm::ReserveAssetDeposited {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		effects: vec![
			Order::BuyExecution {
				fees,
				weight: 0,
				debt: 20,
				halt_on_error: true,
				orders: vec![],
				instructions: vec![],
			},
			Order::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		],
	};

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&MultiLocation::with_parents::<1>(),
		true,
		&mut underpaying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let fees = (MultiLocation::with_parents::<1>(), 1).into();
	let mut paying_message = opaque::Xcm::ReserveAssetDeposited {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		effects: vec![
			Order::BuyExecution {
				fees,
				weight: 0,
				debt: 30,
				halt_on_error: true,
				orders: vec![],
				instructions: vec![],
			},
			Order::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		],
	};

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&X1(Parachain(1)).into(),
		true,
		&mut paying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&MultiLocation::with_parents::<1>(),
		true,
		&mut paying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn paying_reserve_deposit_should_work() {
	AllowPaidFrom::set(vec![MultiLocation::with_parents::<1>()]);
	add_reserve(
		MultiLocation::with_parents::<1>(),
		(MultiLocation::with_parents::<1>(), WildFungible).into(),
	);
	WeightPrice::set((MultiLocation::with_parents::<1>().into(), 1_000_000_000_000));

	let origin = MultiLocation::with_parents::<1>();
	let fees = (MultiLocation::with_parents::<1>(), 30).into();
	let message = Xcm::<TestCall>::ReserveAssetDeposited {
		assets: (MultiLocation::with_parents::<1>(), 100).into(),
		effects: vec![
			Order::<TestCall>::BuyExecution {
				fees,
				weight: 0,
				debt: 30,
				halt_on_error: true,
				orders: vec![],
				instructions: vec![],
			},
			Order::<TestCall>::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		],
	};
	let weight_limit = 50;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(3000), vec![(MultiLocation::with_parents::<1>(), 70).into()]);
}

#[test]
fn transfer_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here.into(), 1000).into());
	// They want to transfer 100 of them to their sibling parachain #2
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		X1(Parachain(1)).into(),
		Xcm::TransferAsset {
			assets: (Here.into(), 100).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: Any }).into(),
		},
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(assets(3), vec![(Here.into(), 100).into()]);
	assert_eq!(assets(1001), vec![(Here.into(), 900).into()]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn reserve_transfer_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, (Here.into(), 1000).into());
	// The remote account owned by gav.
	let three: MultiLocation = X1(AccountIndex64 { index: 3, network: Any }).into();

	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		X1(Parachain(1)).into(),
		Xcm::TransferReserveAsset {
			assets: (Here.into(), 100).into(),
			dest: X1(Parachain(2)).into(),
			effects: vec![Order::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: three.clone(),
			}],
		},
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(assets(1002), vec![(Here.into(), 100).into()]);
	assert_eq!(
		sent_xcm(),
		vec![(
			X1(Parachain(2)).into(),
			Xcm::ReserveAssetDeposited {
				assets: (MultiLocation::with_parents::<1>(), 100).into(),
				effects: vec![Order::DepositAsset {
					assets: All.into(),
					max_assets: 1,
					beneficiary: three
				}],
			}
		)]
	);
}

#[test]
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![MultiLocation::with_parents::<1>()]);

	let origin = MultiLocation::with_parents::<1>();
	let message = Xcm::<TestCall>::Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, None).encode().into(),
	};
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(60));
}

#[test]
fn transacting_should_respect_max_weight_requirement() {
	AllowUnpaidFrom::set(vec![MultiLocation::with_parents::<1>()]);

	let origin = MultiLocation::with_parents::<1>();
	let message = Xcm::<TestCall>::Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 40,
		call: TestCall::Any(50, None).encode().into(),
	};
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Incomplete(60, XcmError::TooMuchWeightRequired));
}

#[test]
fn transacting_should_refund_weight() {
	AllowUnpaidFrom::set(vec![MultiLocation::with_parents::<1>()]);

	let origin = MultiLocation::with_parents::<1>();
	let message = Xcm::<TestCall>::Transact {
		origin_type: OriginKind::Native,
		require_weight_at_most: 50,
		call: TestCall::Any(50, Some(30)).encode().into(),
	};
	let weight_limit = 60;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(40));
}

#[test]
fn paid_transacting_should_refund_payment_for_unused_weight() {
	let one: MultiLocation = X1(AccountIndex64 { index: 1, network: Any }).into();
	AllowPaidFrom::set(vec![one.clone()]);
	add_asset(1, (MultiLocation::with_parents::<1>(), 100).into());
	WeightPrice::set((MultiLocation::with_parents::<1>().into(), 1_000_000_000_000));

	let origin = one.clone();
	let fees = (MultiLocation::with_parents::<1>(), 100).into();
	let message = Xcm::<TestCall>::WithdrawAsset {
		assets: (MultiLocation::with_parents::<1>(), 100).into(), // enough for 100 units of weight.
		effects: vec![
			Order::<TestCall>::BuyExecution {
				fees,
				weight: 70,
				debt: 30,
				halt_on_error: true,
				orders: vec![],
				instructions: vec![Xcm::<TestCall>::Transact {
					origin_type: OriginKind::Native,
					require_weight_at_most: 60,
					// call estimated at 70 but only takes 10.
					call: TestCall::Any(60, Some(10)).encode().into(),
				}],
			},
			Order::<TestCall>::DepositAsset {
				assets: All.into(),
				max_assets: 1,
				beneficiary: one.clone(),
			},
		],
	};
	let weight_limit = 100;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(50));
	assert_eq!(assets(1), vec![(MultiLocation::with_parents::<1>(), 50).into()]);
}

#[test]
fn prepaid_result_of_query_should_get_free_execution() {
	let query_id = 33;
	let origin = MultiLocation::with_parents::<1>();
	// We put this in manually here, but normally this would be done at the point of crafting the message.
	expect_response(query_id, origin.clone());

	let the_response = Response::Assets((MultiLocation::with_parents::<1>(), 100).into());
	let message = Xcm::<TestCall>::QueryResponse { query_id, response: the_response.clone() };
	let weight_limit = 10;

	// First time the response gets through since we're expecting it...
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(response(query_id).unwrap(), the_response);

	// Second time it doesn't, since we're not.
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::Barrier));
}
