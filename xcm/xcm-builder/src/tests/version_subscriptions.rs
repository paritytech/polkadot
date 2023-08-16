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
fn simple_version_subscriptions_should_work() {
	AllowSubsFrom::set(vec![Parent.into()]);

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![
		SetAppendix(Xcm(vec![])),
		SubscribeVersion { query_id: 42, max_response_weight: Weight::from_parts(5000, 5000) },
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(20, 20);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, hash, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![SubscribeVersion {
		query_id: 42,
		max_response_weight: Weight::from_parts(5000, 5000),
	}]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(10, 10);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message.clone(), hash, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(10, 10)));

	assert_eq!(
		SubscriptionRequests::get(),
		vec![(Parent.into(), Some((42, Weight::from_parts(5000, 5000))))]
	);
}

#[test]
fn version_subscription_instruction_should_work() {
	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![
		DescendOrigin(X1(AccountIndex64 { index: 1, network: None })),
		SubscribeVersion { query_id: 42, max_response_weight: Weight::from_parts(5000, 5000) },
	]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(20, 20);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message,
		hash,
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(20, 20), XcmError::BadOrigin));

	let message = Xcm::<TestCall>(vec![
		SetAppendix(Xcm(vec![])),
		SubscribeVersion { query_id: 42, max_response_weight: Weight::from_parts(5000, 5000) },
	]);
	let hash = fake_message_hash(&message);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message,
		hash,
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(20, 20)));

	assert_eq!(
		SubscriptionRequests::get(),
		vec![(Parachain(1000).into(), Some((42, Weight::from_parts(5000, 5000))))]
	);
}

#[test]
fn simple_version_unsubscriptions_should_work() {
	AllowSubsFrom::set(vec![Parent.into()]);

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![SetAppendix(Xcm(vec![])), UnsubscribeVersion]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(20, 20);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message, hash, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let origin = Parachain(1000);
	let message = Xcm::<TestCall>(vec![UnsubscribeVersion]);
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(10, 10);
	let r = XcmExecutor::<TestConfig>::execute_xcm(origin, message.clone(), hash, weight_limit);
	assert_eq!(r, Outcome::Error(XcmError::Barrier));

	let r = XcmExecutor::<TestConfig>::execute_xcm(Parent, message, hash, weight_limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(10, 10)));

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
	let hash = fake_message_hash(&message);
	let weight_limit = Weight::from_parts(20, 20);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message,
		hash,
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(20, 20), XcmError::BadOrigin));

	// Fine to do it when origin is untouched.
	let message = Xcm::<TestCall>(vec![SetAppendix(Xcm(vec![])), UnsubscribeVersion]);
	let hash = fake_message_hash(&message);
	let r = XcmExecutor::<TestConfig>::execute_xcm_in_credit(
		origin,
		message,
		hash,
		weight_limit,
		weight_limit,
	);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(20, 20)));

	assert_eq!(SubscriptionRequests::get(), vec![(Parachain(1000).into(), None)]);
	assert_eq!(sent_xcm(), vec![]);
}
