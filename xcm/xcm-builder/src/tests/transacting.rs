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

use super::*;

#[test]
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_parts(50, 50),
		call: TestCall::Any(Weight::from_parts(50, 50), None).encode().into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(60, 60);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(60, 60)));
}

#[test]
fn transacting_should_respect_max_weight_requirement() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_parts(40, 40),
		call: TestCall::Any(Weight::from_parts(50, 50), None).encode().into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(60, 60);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(50, 50), XcmError::MaxWeightInvalid));
}

#[test]
fn transacting_should_refund_weight() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_parts(50, 50),
		call: TestCall::Any(Weight::from_parts(50, 50), Some(Weight::from_parts(30, 30)))
			.encode()
			.into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(60, 60);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(40, 40)));
}

#[test]
fn paid_transacting_should_refund_payment_for_unused_weight() {
	let one: MultiLocation = AccountIndex64 { index: 1, network: None }.into();
	AllowPaidFrom::set(vec![one]);
	add_asset(AccountIndex64 { index: 1, network: None }, (Parent, 200u128));
	WeightPrice::set((Parent.into(), 1_000_000_000_000, 1024 * 1024));

	let origin = one;
	let fees = (Parent, 200u128).into();
	let message = Xcm::<TestCall>(vec![
		WithdrawAsset((Parent, 200u128).into()), // enough for 200 units of weight.
		BuyExecution { fees, weight_limit: Limited(Weight::from_parts(100, 100)) },
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			// call estimated at 50 but only takes 10.
			call: TestCall::Any(Weight::from_parts(50, 50), Some(Weight::from_parts(10, 10)))
				.encode()
				.into(),
		},
		RefundSurplus,
		DepositAsset { assets: AllCounted(1).into(), beneficiary: one },
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(100, 100);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(60, 60)));
	assert_eq!(
		asset_list(AccountIndex64 { index: 1, network: None }),
		vec![(Parent, 80u128).into()]
	);
}

#[test]
fn report_successful_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::Any(Weight::from_parts(50, 50), None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_parts(5000, 5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(70, 70)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(MaybeErrorCode::Success),
		query_id: 42,
		max_weight: Weight::from_parts(5000, 5000),
		querier: Some(Here.into()),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![(Parent.into(), expected_msg, expected_hash)]);
}

#[test]
fn report_failed_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::OnlyRoot(Weight::from_parts(50, 50), None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_parts(5000, 5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(70, 70)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(vec![2].into()),
		query_id: 42,
		max_weight: Weight::from_parts(5000, 5000),
		querier: Some(Here.into()),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![(Parent.into(), expected_msg, expected_hash)]);
}

#[test]
fn expect_successful_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::Any(Weight::from_parts(50, 50), None).encode().into(),
		},
		ExpectTransactStatus(MaybeErrorCode::Success),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(70, 70)));

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::OnlyRoot(Weight::from_parts(50, 50), None).encode().into(),
		},
		ExpectTransactStatus(MaybeErrorCode::Success),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(70, 70), XcmError::ExpectationFalse));
}

#[test]
fn expect_failed_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::OnlyRoot(Weight::from_parts(50, 50), None).encode().into(),
		},
		ExpectTransactStatus(vec![2].into()),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(70, 70)));

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::Any(Weight::from_parts(50, 50), None).encode().into(),
		},
		ExpectTransactStatus(vec![2].into()),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(70, 70);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(70, 70), XcmError::ExpectationFalse));
}

#[test]
fn clear_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_parts(50, 50),
			call: TestCall::OnlyRoot(Weight::from_parts(50, 50), None).encode().into(),
		},
		ClearTransactStatus,
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_parts(5000, 5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(80, 80);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(80, 80)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(MaybeErrorCode::Success),
		query_id: 42,
		max_weight: Weight::from_parts(5000, 5000),
		querier: Some(Here.into()),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![(Parent.into(), expected_msg, expected_hash)]);
}
