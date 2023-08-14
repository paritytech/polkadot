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

use sp_std::convert::Infallible;
use xcm::prelude::*;

#[derive(Debug)]
pub enum LockError {
	NotApplicable,
	WouldClobber,
	BadOrigin,
	NotLocked,
	NotEnoughLocked,
	Unimplemented,
	NotTrusted,
	BadOwner,
	UnknownAsset,
	AssetNotOwned,
	NoResources,
	UnexpectedState,
	InUse,
}

impl From<LockError> for XcmError {
	fn from(e: LockError) -> XcmError {
		use LockError::*;
		match e {
			NotApplicable => XcmError::AssetNotFound,
			BadOrigin => XcmError::BadOrigin,
			WouldClobber | NotLocked | NotEnoughLocked | Unimplemented | NotTrusted |
			BadOwner | UnknownAsset | AssetNotOwned | NoResources | UnexpectedState | InUse =>
				XcmError::LockError,
		}
	}
}

pub trait Enact {
	/// Enact a lock. This should generally be infallible if called immediately after being
	/// received.
	fn enact(self) -> Result<(), LockError>;
}

impl Enact for Infallible {
	fn enact(self) -> Result<(), LockError> {
		unreachable!()
	}
}

/// Define a handler for notification of an asset being locked and for the unlock instruction.
pub trait AssetLock {
	/// `Enact` implementer for `prepare_lock`. This type may be dropped safely to avoid doing the
	/// lock.
	type LockTicket: Enact;

	/// `Enact` implementer for `prepare_unlock`. This type may be dropped safely to avoid doing the
	/// unlock.
	type UnlockTicket: Enact;

	/// `Enact` implementer for `prepare_reduce_unlockable`. This type may be dropped safely to
	/// avoid doing the unlock.
	type ReduceTicket: Enact;

	/// Prepare to lock an asset. On success, a `Self::LockTicket` it returned, which can be used
	/// to actually enact the lock.
	///
	/// WARNING: Don't call this with an undropped instance of `Self::LockTicket` or
	/// `Self::UnlockTicket`.
	fn prepare_lock(
		unlocker: MultiLocation,
		asset: MultiAsset,
		owner: MultiLocation,
	) -> Result<Self::LockTicket, LockError>;

	/// Prepare to unlock an asset. On success, a `Self::UnlockTicket` it returned, which can be
	/// used to actually enact the lock.
	///
	/// WARNING: Don't call this with an undropped instance of `Self::LockTicket` or
	/// `Self::UnlockTicket`.
	fn prepare_unlock(
		locker: MultiLocation,
		asset: MultiAsset,
		owner: MultiLocation,
	) -> Result<Self::UnlockTicket, LockError>;

	/// Handler for when a location reports to us that an asset has been locked for us to unlock
	/// at a later stage.
	///
	/// If there is no way to handle the lock report, then this should return an error so that the
	/// sending chain can ensure the lock does not remain.
	///
	/// We should only act upon this message if we believe that the `origin` is honest.
	fn note_unlockable(
		locker: MultiLocation,
		asset: MultiAsset,
		owner: MultiLocation,
	) -> Result<(), LockError>;

	/// Handler for when an owner wishes to unlock an asset on a remote chain.
	///
	/// Returns a ticket which can be used to actually note the reduction in unlockable assets that
	/// `owner` commands on `locker`.
	///
	/// WARNING: Don't call this with an undropped instance of `Self::ReduceTicket`.
	fn prepare_reduce_unlockable(
		locker: MultiLocation,
		asset: MultiAsset,
		owner: MultiLocation,
	) -> Result<Self::ReduceTicket, LockError>;
}

impl AssetLock for () {
	type LockTicket = Infallible;
	type UnlockTicket = Infallible;
	type ReduceTicket = Infallible;
	fn prepare_lock(
		_: MultiLocation,
		_: MultiAsset,
		_: MultiLocation,
	) -> Result<Self::LockTicket, LockError> {
		Err(LockError::NotApplicable)
	}
	fn prepare_unlock(
		_: MultiLocation,
		_: MultiAsset,
		_: MultiLocation,
	) -> Result<Self::UnlockTicket, LockError> {
		Err(LockError::NotApplicable)
	}
	fn note_unlockable(_: MultiLocation, _: MultiAsset, _: MultiLocation) -> Result<(), LockError> {
		Err(LockError::NotApplicable)
	}
	fn prepare_reduce_unlockable(
		_: MultiLocation,
		_: MultiAsset,
		_: MultiLocation,
	) -> Result<Self::ReduceTicket, LockError> {
		Err(LockError::NotApplicable)
	}
}
