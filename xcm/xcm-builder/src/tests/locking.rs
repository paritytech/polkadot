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
use LockTraceItem::*;

#[test]
fn lock_roundtrip_should_work() {
	// Account #3 and Parachain #1 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000u128));
	// Sending a message costs 10 parent-tokens.
	set_send_price((Parent, 10u128));

	// They want to lock 100 of the native parent tokens to be unlocked only by Parachain #1.
	let message = Xcm(vec![
		WithdrawAsset((Parent, 100u128).into()),
		SetAppendix(
			vec![DepositAsset { assets: AllCounted(2).into(), beneficiary: (3u64,).into() }].into(),
		),
		LockAsset { asset: (Parent, 100u128).into(), unlocker: (Parent, Parachain(1)).into() },
	]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Complete(Weight::from_parts(40, 40)));
	assert_eq!(asset_list((3u64,)), vec![(Parent, 990u128).into()]);

	let expected_msg = Xcm::<()>(vec![NoteUnlockable {
		owner: (Parent, Parachain(42), 3u64).into(),
		asset: (Parent, 100u128).into(),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![((Parent, Parachain(1)).into(), expected_msg, expected_hash)]);
	assert_eq!(
		take_lock_trace(),
		vec![Lock {
			asset: (Parent, 100u128).into(),
			owner: (3u64,).into(),
			unlocker: (Parent, Parachain(1)).into(),
		}]
	);

	// Now we'll unlock it.
	let message =
		Xcm(vec![UnlockAsset { asset: (Parent, 100u128).into(), target: (3u64,).into() }]);
	let hash = fake_message_hash(&message);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		message,
		hash,
		Weight::from_parts(50, 50),
	);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(10, 10)));
}

#[test]
fn auto_fee_paying_should_work() {
	// Account #3 and Parachain #1 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into()]);
	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000u128));
	// Sending a message costs 10 parent-tokens.
	set_send_price((Parent, 10u128));

	// They want to lock 100 of the native parent tokens to be unlocked only by Parachain #1.
	let message = Xcm(vec![
		SetFeesMode { jit_withdraw: true },
		LockAsset { asset: (Parent, 100u128).into(), unlocker: (Parent, Parachain(1)).into() },
	]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Complete(Weight::from_parts(20, 20)));
	assert_eq!(asset_list((3u64,)), vec![(Parent, 990u128).into()]);
}

#[test]
fn lock_should_fail_correctly() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);

	// #3 wants to lock 100 of the native parent tokens to be unlocked only by parachain ../#1,
	// but they don't have any.
	let message = Xcm(vec![LockAsset {
		asset: (Parent, 100u128).into(),
		unlocker: (Parent, Parachain(1)).into(),
	}]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(10, 10), XcmError::LockError));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);

	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000u128));
	// But we require a price to be paid for the sending
	set_send_price((Parent, 10u128));

	// #3 wants to lock 100 of the native parent tokens to be unlocked only by parachain ../#1,
	// but there's nothing to pay the fees for sending the notification message.
	let message = Xcm(vec![LockAsset {
		asset: (Parent, 100u128).into(),
		unlocker: (Parent, Parachain(1)).into(),
	}]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(10, 10), XcmError::NotHoldingFees));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);
}

#[test]
fn remote_unlock_roundtrip_should_work() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000u128));
	// Sending a message costs 10 parent-tokens.
	set_send_price((Parent, 10u128));

	// We have been told by Parachain #1 that Account #3 has locked funds which we can unlock.
	// Previously, we must have sent a LockAsset instruction to Parachain #1.
	// This caused Parachain #1 to send us the NoteUnlockable instruction.
	let message =
		Xcm(vec![NoteUnlockable { asset: (Parent, 100u128).into(), owner: (3u64,).into() }]);
	let hash = fake_message_hash(&message);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		message,
		hash,
		Weight::from_parts(50, 50),
	);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(10, 10)));
	assert_eq!(
		take_lock_trace(),
		vec![Note {
			asset: (Parent, 100u128).into(),
			owner: (3u64,).into(),
			locker: (Parent, Parachain(1)).into(),
		}]
	);

	// Let's request those funds be unlocked.
	let message = Xcm(vec![
		WithdrawAsset((Parent, 100u128).into()),
		SetAppendix(
			vec![DepositAsset { assets: AllCounted(2).into(), beneficiary: (3u64,).into() }].into(),
		),
		RequestUnlock { asset: (Parent, 100u128).into(), locker: (Parent, Parachain(1)).into() },
	]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Complete(Weight::from_parts(40, 40)));
	assert_eq!(asset_list((3u64,)), vec![(Parent, 990u128).into()]);

	let expected_msg = Xcm::<()>(vec![UnlockAsset {
		target: (Parent, Parachain(42), 3u64).into(),
		asset: (Parent, 100u128).into(),
	}]);
	let expected_hash = fake_message_hash(&expected_msg);
	assert_eq!(sent_xcm(), vec![((Parent, Parachain(1)).into(), expected_msg, expected_hash)]);
	assert_eq!(
		take_lock_trace(),
		vec![Reduce {
			asset: (Parent, 100u128).into(),
			owner: (3u64,).into(),
			locker: (Parent, Parachain(1)).into(),
		}]
	);
}

#[test]
fn remote_unlock_should_fail_correctly() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// But we require a price to be paid for the sending
	set_send_price((Parent, 10u128));

	// We want to unlock 100 of the native parent tokens which were locked for us on parachain.
	// This won't work as we don't have any record of them being locked for us.
	// No message will be sent and no lock records changed.
	let message = Xcm(vec![RequestUnlock {
		asset: (Parent, 100u128).into(),
		locker: (Parent, Parachain(1)).into(),
	}]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(10, 10), XcmError::LockError));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);

	// We have been told by Parachain #1 that Account #3 has locked funds which we can unlock.
	let message =
		Xcm(vec![NoteUnlockable { asset: (Parent, 100u128).into(), owner: (3u64,).into() }]);
	let hash = fake_message_hash(&message);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		message,
		hash,
		Weight::from_parts(50, 50),
	);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(10, 10)));
	let _discard = take_lock_trace();

	// We want to unlock 100 of the native parent tokens which were locked for us on parachain.
	// This won't work now as we don't have the funds to send the onward message.
	// No message will be sent and no lock records changed.
	let message = Xcm(vec![RequestUnlock {
		asset: (Parent, 100u128).into(),
		locker: (Parent, Parachain(1)).into(),
	}]);
	let hash = fake_message_hash(&message);
	let r =
		XcmExecutor::<TestConfig>::execute_xcm((3u64,), message, hash, Weight::from_parts(50, 50));
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(10, 10), XcmError::NotHoldingFees));

	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);
}
