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

use super::*;
use super::mock::*;
use {MultiAsset::*, Option::None};
use xcm::v0::{Order, NetworkId::Any, Outcome, Response, ExecuteXcm};
use xcm_executor::{XcmExecutor, Config, traits::*};

#[test]
fn basic_setup_works() {
	add_reserve(
		MultiLocation::new(1, Null).unwrap(),
		AllConcreteFungible { id: MultiLocation::new(1, Null).unwrap() },
	);
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 },
		&MultiLocation::new(1, Null).unwrap(),
	));

	assert_eq!(to_account(X1(Parachain(1)).into()), Ok(1001));
	assert_eq!(to_account(X1(Parachain(50)).into()), Ok(1050));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(1))).unwrap()), Ok(2001));
	assert_eq!(to_account(MultiLocation::new(1, X1(Parachain(50))).unwrap()), Ok(2050));
	assert_eq!(to_account(X1(AccountIndex64{index:1, network:Any}).into()), Ok(1));
	assert_eq!(to_account(X1(AccountIndex64{index:42, network:Any}).into()), Ok(42));
	assert_eq!(to_account(Null.into()), Ok(3000));
}

#[test]
fn weigher_should_work() {
	let mut message = opaque::Xcm::ReserveAssetDeposit {
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		effects: vec![
			Order::BuyExecution { fees: All, weight: 0, debt: 30, halt_on_error: true, xcm: vec![] },
			Order::DepositAsset { assets: vec![All], dest: Null.into() },
		],
	}.into();
	assert_eq!(<TestConfig as Config>::Weigher::shallow(&mut message), Ok(30));
}

#[test]
fn take_weight_credit_barrier_should_work() {
	let mut message = opaque::Xcm::TransferAsset {
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		dest: Null.into(),
	};

	let mut weight_credit = 10;
	let r = TakeWeightCredit::should_execute(
		&MultiLocation::new(1, Null).unwrap(),
		true,
		&mut message,
		10,
		&mut weight_credit,
	);
	assert_eq!(r, Ok(()));
	assert_eq!(weight_credit, 0);

	let r = TakeWeightCredit::should_execute(
		&MultiLocation::new(1, Null).unwrap(),
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
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		dest: Null.into(),
	};

	AllowUnpaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&X1(Parachain(1)).into(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&MultiLocation::new(1, Null).unwrap(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn allow_paid_should_work() {
	AllowPaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);

	let mut message = opaque::Xcm::TransferAsset {
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		dest: Null.into(),
	};

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&X1(Parachain(1)).into(),
		true,
		&mut message,
		10,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let mut underpaying_message = opaque::Xcm::ReserveAssetDeposit {
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		effects: vec![
			Order::BuyExecution { fees: All, weight: 0, debt: 20, halt_on_error: true, xcm: vec![] },
			Order::DepositAsset { assets: vec![All], dest: Null.into() },
		],
	};

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&MultiLocation::new(1, Null).unwrap(),
		true,
		&mut underpaying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Err(()));

	let mut paying_message = opaque::Xcm::ReserveAssetDeposit {
		assets: vec![ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 }],
		effects: vec![
			Order::BuyExecution { fees: All, weight: 0, debt: 30, halt_on_error: true, xcm: vec![] },
			Order::DepositAsset { assets: vec![All], dest: Null.into() },
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
		&MultiLocation::new(1, Null).unwrap(),
		true,
		&mut paying_message,
		30,
		&mut 0,
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn paying_reserve_deposit_should_work() {
	AllowPaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);
	add_reserve(
		MultiLocation::new(1, Null).unwrap(),
		AllConcreteFungible { id: MultiLocation::new(1, Null).unwrap() },
	);
	WeightPrice::set((MultiLocation::new(1, Null).unwrap(), 1_000_000_000_000));

	let origin = MultiLocation::new(1, Null).unwrap();
	let message = Xcm::<TestCall>::ReserveAssetDeposit {
		assets: vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 } ],
		effects: vec![
			Order::<TestCall>::BuyExecution { fees: All, weight: 0, debt: 30, halt_on_error: true, xcm: vec![] },
			Order::<TestCall>::DepositAsset { assets: vec![ All ], dest: Null.into() },
		],
	};
	let weight_limit = 50;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(assets(3000), vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 70 } ]);
}

#[test]
fn transfer_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![ X1(Parachain(1)).into() ]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, ConcreteFungible { id: Null.into(), amount: 1000 });
	// They want to transfer 100 of them to their sibling parachain #2
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		X1(Parachain(1)).into(),
		Xcm::TransferAsset {
			assets: vec![ ConcreteFungible { id: Null.into(), amount: 100 } ],
			dest: X1(AccountIndex64{index:3, network:Any}).into(),
		},
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(assets(3), vec![ ConcreteFungible { id: Null.into(), amount: 100 } ]);
	assert_eq!(assets(1001), vec![ ConcreteFungible { id: Null.into(), amount: 900 } ]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn reserve_transfer_should_work() {
	AllowUnpaidFrom::set(vec![ X1(Parachain(1)).into() ]);
	// Child parachain #1 owns 1000 tokens held by us in reserve.
	add_asset(1001, ConcreteFungible { id: Null.into(), amount: 1000 });
	// The remote account owned by gav.
	let three: MultiLocation = X1(AccountIndex64{index:3, network:Any}).into();

	// They want to transfer 100 of our native asset from sovereign account of parachain #1 into #2
	// and let them know to hand it to account #3.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		X1(Parachain(1)).into(),
		Xcm::TransferReserveAsset {
			assets: vec![ ConcreteFungible { id: Null.into(), amount: 100 } ],
			dest: X1(Parachain(2)).into(),
			effects: vec![ Order::DepositAsset { assets: vec![ All ], dest: three.clone() } ],
		},
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(assets(1002), vec![ ConcreteFungible { id: Null.into(), amount: 100 } ]);
	assert_eq!(sent_xcm(), vec![(
		X1(Parachain(2)).into(),
		Xcm::ReserveAssetDeposit {
			assets: vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 } ],
			effects: vec![ Order::DepositAsset { assets: vec![ All ], dest: three } ],
		})
	]);
}

#[test]
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);

	let origin = MultiLocation::new(1, Null).unwrap();
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
	AllowUnpaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);

	let origin = MultiLocation::new(1, Null).unwrap();
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
	AllowUnpaidFrom::set(vec![ MultiLocation::new(1, Null).unwrap() ]);

	let origin = MultiLocation::new(1, Null).unwrap();
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
	let one: MultiLocation = X1(AccountIndex64{index:1, network:Any}).into();
	AllowPaidFrom::set(vec![ one.clone() ]);
	add_asset(1, ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 });
	WeightPrice::set((MultiLocation::new(1, Null).unwrap(), 1_000_000_000_000));

	let origin = one.clone();
	let message = Xcm::<TestCall>::WithdrawAsset {
		assets: vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 } ],	// enough for 100 units of weight.
		effects: vec![
			Order::<TestCall>::BuyExecution { fees: All, weight: 70, debt: 30, halt_on_error: true, xcm: vec![
				Xcm::<TestCall>::Transact {
					origin_type: OriginKind::Native,
					require_weight_at_most: 60,
					// call estimated at 70 but only takes 10.
					call: TestCall::Any(60, Some(10)).encode().into(),
				}
			] },
			Order::<TestCall>::DepositAsset { assets: vec![ All ], dest: one.clone() },
		],
	};
	let weight_limit = 100;
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, weight_limit);
	assert_eq!(r, Outcome::Complete(50));
	assert_eq!(assets(1), vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 50 } ]);
}

#[test]
fn prepaid_result_of_query_should_get_free_execution() {
	let query_id = 33;
	let origin = MultiLocation::new(1, Null).unwrap();
	// We put this in manually here, but normally this would be done at the point of crafting the message.
	expect_response(query_id, origin.clone());

	let the_response = Response::Assets(
		vec![ ConcreteFungible { id: MultiLocation::new(1, Null).unwrap(), amount: 100 } ],
	);
	let message = Xcm::<TestCall>::QueryResponse {
		query_id,
		response: the_response.clone(),
	};
	let weight_limit = 10;

	// First time the response gets through since we're expecting it...
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(response(query_id).unwrap(), the_response);

	// Second time it doesn't, since we're not.
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin.clone(), message.clone(), weight_limit);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::Barrier));
}
