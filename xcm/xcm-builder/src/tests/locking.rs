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
use LockTraceItem::*;

#[test]
fn lock_roundtrip_should_work() {
	// Account #3 and Parachain #1 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000));

	// They want to lock 100 of the native parent tokens to be unlocked only by Parachain #1.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![LockAsset {
			asset: (Parent, 100).into(),
			unlocker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(
		sent_xcm(),
		vec![(
			(Parent, Parachain(1)).into(),
			Xcm::<()>(vec![
				NoteUnlockable { owner: (3u64,).into(), asset: (Parent, 100).into() },
			]),
		)]
	);
	assert_eq!(take_lock_trace(), vec![Lock {
		asset: (Parent, 100).into(),
		owner: (3u64,).into(),
		unlocker: (Parent, Parachain(1)).into(),
	}]);

	// Now we'll unlock it.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		Xcm(vec![UnlockAsset {
			asset: (Parent, 100).into(),
			target: (3u64,).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
}

#[test]
fn lock_should_fail_correctly() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);

	// #3 wants to lock 100 of the native parent tokens to be unlocked only by parachain ../#1,
	// but they don't have any.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![LockAsset {
			asset: (Parent, 100).into(),
			unlocker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::LockError));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);

	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000));
	// But we require a price to be paid for the sending
	set_send_price((Parent, 10));

	// #3 wants to lock 100 of the native parent tokens to be unlocked only by parachain ../#1,
	// but there's nothing to pay the fees for sending the notification message.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![LockAsset {
			asset: (Parent, 100).into(),
			unlocker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NotHoldingFees));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);
}

#[test]
fn remote_unlock_roundtrip_should_work() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// Account #3 owns 1000 native parent tokens.
	add_asset((3u64,), (Parent, 1000));

	// We have been told by Parachain #1 that Account #3 has locked funds which we can unlock.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		Xcm(vec![NoteUnlockable {
			asset: (Parent, 100).into(),
			owner: (3u64,).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(take_lock_trace(), vec![Note {
		asset: (Parent, 100).into(),
		owner: (3u64,).into(),
		locker: (Parent, Parachain(1)).into(),
	}]);

	// Let's request those funds be unlocked.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![RequestUnlock {
			asset: (Parent, 100).into(),
			locker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));

	assert_eq!(
		sent_xcm(),
		vec![(
			(Parent, Parachain(1)).into(),
			Xcm::<()>(vec![
				UnlockAsset { target: (3u64,).into(), asset: (Parent, 100).into() },
			]),
		)]
	);
	assert_eq!(take_lock_trace(), vec![Reduce {
		asset: (Parent, 100).into(),
		owner: (3u64,).into(),
		locker: (Parent, Parachain(1)).into(),
	}]);
}

#[test]
fn remote_unlock_should_fail_correctly() {
	// Account #3 can execute for free
	AllowUnpaidFrom::set(vec![(3u64,).into(), (Parent, Parachain(1)).into()]);
	// But we require a price to be paid for the sending
	set_send_price((Parent, 10));

	// We want to unlock 100 of the native parent tokens which were locked for us on parachain.
	// This won't work as we don't have any record of them being locked for us.
	// No message will be sent and no lock records changed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![RequestUnlock {
			asset: (Parent, 100).into(),
			locker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::LockError));
	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);

	// We have been told by Parachain #1 that Account #3 has locked funds which we can unlock.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(Parent, Parachain(1)),
		Xcm(vec![NoteUnlockable {
			asset: (Parent, 100).into(),
			owner: (3u64,).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	let _discard = take_lock_trace();

	// We want to unlock 100 of the native parent tokens which were locked for us on parachain.
	// This won't work now as we don't have the funds to send the onward message.
	// No message will be sent and no lock records changed.
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		(3u64,),
		Xcm(vec![RequestUnlock {
			asset: (Parent, 100).into(),
			locker: (Parent, Parachain(1)).into(),
		}]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NotHoldingFees));

	assert_eq!(sent_xcm(), vec![]);
	assert_eq!(take_lock_trace(), vec![]);
}
