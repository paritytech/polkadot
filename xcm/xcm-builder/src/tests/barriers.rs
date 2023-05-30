// Copyright (C) Parity Technologies (UK) Ltd.
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

use xcm_executor::traits::Properties;

use super::*;

fn props(weight_credit: Weight) -> Properties {
	Properties { weight_credit, message_id: None }
}

#[test]
fn take_weight_credit_barrier_should_work() {
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);
	let mut properties = props(Weight::from_parts(10, 10));
	let r = TakeWeightCredit::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(10, 10),
		&mut properties,
	);
	assert_eq!(r, Ok(()));
	assert_eq!(properties.weight_credit, Weight::zero());

	let r = TakeWeightCredit::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(10, 10),
		&mut properties,
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(10, 10))));
	assert_eq!(properties.weight_credit, Weight::zero());
}

#[test]
fn computed_origin_should_work() {
	let mut message = Xcm::<()>(vec![
		UniversalOrigin(GlobalConsensus(Kusama)),
		DescendOrigin(Parachain(100).into()),
		DescendOrigin(PalletInstance(69).into()),
		WithdrawAsset((Parent, 100).into()),
		BuyExecution {
			fees: (Parent, 100).into(),
			weight_limit: Limited(Weight::from_parts(100, 100)),
		},
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
		Weight::from_parts(100, 100),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let r = WithComputedOrigin::<
		AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
		ExecutorUniversalLocation,
		ConstU32<2>,
	>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(100, 100),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let r = WithComputedOrigin::<
		AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
		ExecutorUniversalLocation,
		ConstU32<5>,
	>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(100, 100),
		&mut props(Weight::zero()),
	);
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
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let r = AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Ok(()));
}

#[test]
fn allow_explicit_unpaid_should_work() {
	let mut bad_message1 =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);

	let mut bad_message2 = Xcm::<()>(vec![
		UnpaidExecution {
			weight_limit: Limited(Weight::from_parts(10, 10)),
			check_origin: Some(Parent.into()),
		},
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);

	let mut good_message = Xcm::<()>(vec![
		UnpaidExecution {
			weight_limit: Limited(Weight::from_parts(20, 20)),
			check_origin: Some(Parent.into()),
		},
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);

	AllowExplicitUnpaidFrom::set(vec![Parent.into()]);

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parachain(1).into(),
		good_message.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parent.into(),
		bad_message1.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(20, 20))));

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parent.into(),
		bad_message2.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(20, 20))));

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parent.into(),
		good_message.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Ok(()));

	let mut message_with_different_weight_parts = Xcm::<()>(vec![
		UnpaidExecution {
			weight_limit: Limited(Weight::from_parts(20, 10)),
			check_origin: Some(Parent.into()),
		},
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parent.into(),
		message_with_different_weight_parts.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(20, 20))));

	let r = AllowExplicitUnpaidExecutionFrom::<IsInVec<AllowExplicitUnpaidFrom>>::should_execute(
		&Parent.into(),
		message_with_different_weight_parts.inner_mut(),
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
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
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let fees = (Parent, 1).into();
	let mut underpaying_message = Xcm::<()>(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(Weight::from_parts(20, 20)) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		underpaying_message.inner_mut(),
		Weight::from_parts(30, 30),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(30, 30))));

	let fees = (Parent, 1).into();
	let mut paying_message = Xcm::<()>(vec![
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(Weight::from_parts(30, 30)) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parachain(1).into(),
		paying_message.inner_mut(),
		Weight::from_parts(30, 30),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Unsupported));

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		paying_message.inner_mut(),
		Weight::from_parts(30, 30),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Ok(()));

	let fees = (Parent, 1).into();
	let mut paying_message_with_different_weight_parts = Xcm::<()>(vec![
		WithdrawAsset((Parent, 100).into()),
		BuyExecution { fees, weight_limit: Limited(Weight::from_parts(20, 10)) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		paying_message_with_different_weight_parts.inner_mut(),
		Weight::from_parts(20, 20),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Overweight(Weight::from_parts(20, 20))));

	let r = AllowTopLevelPaidExecutionFrom::<IsInVec<AllowPaidFrom>>::should_execute(
		&Parent.into(),
		paying_message_with_different_weight_parts.inner_mut(),
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Ok(()))
}

#[test]
fn suspension_should_work() {
	TestSuspender::set_suspended(true);
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);
	let r = RespectSuspension::<AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>, TestSuspender>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Err(ProcessMessageError::Yield));

	TestSuspender::set_suspended(false);
	let mut message =
		Xcm::<()>(vec![TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() }]);
	let r = RespectSuspension::<AllowUnpaidExecutionFrom::<IsInVec<AllowUnpaidFrom>>, TestSuspender>::should_execute(
		&Parent.into(),
		message.inner_mut(),
		Weight::from_parts(10, 10),
		&mut props(Weight::zero()),
	);
	assert_eq!(r, Ok(()));
}
