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
fn transacting_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_ref_time(50),
		call: TestCall::Any(Weight::from_ref_time(50), None).encode().into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(60).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(60).set_proof_size(DEFAULT_PROOF_SIZE)));
}

#[test]
fn transacting_should_respect_max_weight_requirement() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_ref_time(40),
		call: TestCall::Any(Weight::from_ref_time(50), None).encode().into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(60).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_ref_time(50).set_proof_size(DEFAULT_PROOF_SIZE), XcmError::MaxWeightInvalid));
}

#[test]
fn transacting_should_refund_weight() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![Transact {
		origin_kind: OriginKind::Native,
		require_weight_at_most: Weight::from_ref_time(50),
		call: TestCall::Any(Weight::from_ref_time(50), Some(Weight::from_ref_time(30))).encode().into(),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(60).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(40).set_proof_size(DEFAULT_PROOF_SIZE)));
}

#[test]
fn paid_transacting_should_refund_payment_for_unused_weight() {
	let one: MultiLocation = AccountIndex64 { index: 1, network: None }.into();
	AllowPaidFrom::set(vec![one.clone()]);
	add_asset(AccountIndex64 { index: 1, network: None }, (Parent, 100u128));
	WeightPrice::set((Parent.into(), 1_000_000_000_000));

	let origin = one.clone();
	let fees = (Parent, 100u128).into();
	let message = Xcm::<TestCall>(vec![
		WithdrawAsset((Parent, 100u128).into()), // enough for 100 units of weight.
		BuyExecution { fees, weight_limit: Limited(Weight::from_ref_time(100)) },
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_ref_time(50),
			// call estimated at 50 but only takes 10.
			call: TestCall::Any(Weight::from_ref_time(50), Some(Weight::from_ref_time(10))).encode().into(),
		},
		RefundSurplus,
		DepositAsset { assets: AllCounted(1).into(), beneficiary: one.clone() },
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(100).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(60).set_proof_size(DEFAULT_PROOF_SIZE)));
	assert_eq!(
		asset_list(AccountIndex64 { index: 1, network: None }),
		vec![(Parent, 40u128).into()]
	);
}

#[test]
fn report_successful_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_ref_time(50),
			call: TestCall::Any(Weight::from_ref_time(50), None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_ref_time(5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(70).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(70).set_proof_size(DEFAULT_PROOF_SIZE)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(MaybeErrorCode::Success),
		query_id: 42,
		max_weight: Weight::from_ref_time(5000),
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
			require_weight_at_most: Weight::from_ref_time(50),
			call: TestCall::OnlyRoot(Weight::from_ref_time(50), None).encode().into(),
		},
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_ref_time(5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(70).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(70).set_proof_size(DEFAULT_PROOF_SIZE)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(MaybeErrorCode::Error(vec![2])),
		query_id: 42,
		max_weight: Weight::from_ref_time(5000),
		querier: Some(Here.into()),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![(Parent.into(), expected_msg, expected_hash)]);
}

#[test]
fn clear_transact_status_should_work() {
	AllowUnpaidFrom::set(vec![Parent.into()]);

	let message = Xcm::<TestCall>(vec![
		Transact {
			origin_kind: OriginKind::Native,
			require_weight_at_most: Weight::from_ref_time(50),
			call: TestCall::OnlyRoot(Weight::from_ref_time(50), None).encode().into(),
		},
		ClearTransactStatus,
		ReportTransactStatus(QueryResponseInfo {
			destination: Parent.into(),
			query_id: 42,
			max_weight: Weight::from_ref_time(5000),
		}),
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_ref_time(80).set_proof_size(DEFAULT_PROOF_SIZE);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_ref_time(80).set_proof_size(DEFAULT_PROOF_SIZE)));
	let expected_msg = Xcm(vec![QueryResponse {
		response: Response::DispatchResult(MaybeErrorCode::Success),
		query_id: 42,
		max_weight: Weight::from_ref_time(5000),
		querier: Some(Here.into()),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![(Parent.into(), expected_msg, expected_hash)]);
}
