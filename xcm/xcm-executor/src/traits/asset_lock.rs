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

use xcm::prelude::*;

pub enum LockError {
	NotApplicable,
	WouldClobber,
	BadOrigin,
	NotLocked,
	Unimplemented,
	NotTrusted,
	BadOwner,
	UnknownAsset,
}

impl From<LockError> for XcmError {
	fn from(e: LockError) -> XcmError {
		use LockError::*;
		match e {
			NotApplicable => XcmError::AssetNotFound,
			BadOrigin => XcmError::BadOrigin,
			WouldClobber => todo!(),
			NotLocked => todo!(),
			Unimplemented => todo!(),
			NotTrusted => todo!(),
			BadOwner => todo!(),
			UnknownAsset => todo!(),
		}
	}
}

/// Define a handler for notification of an asset being locked and for the unlock instruction.
pub trait LockAsset {
	/// Handler for when a location reports to us that an asset has been locked for us to unlock
	/// at a later stage.
	///
	/// If there is no way to handle the lock report, then this should return an error so that the
	/// sending chain can ensure the lock does not remain.
	///
	/// We should only act upon this message if we believe that the `origin` is honest.
	fn note_asset_locked(origin: MultiLocation, asset: MultiAsset, owner: MultiLocation) -> Result<(), LockError>;

	/// Handler for unlocking an asset.
	fn unlock_asset(origin: MultiLocation, asset: MultiAsset, owner: MultiLocation) -> Result<(), LockError>;
}

impl LockAsset for () {
	fn note_asset_locked(_: MultiLocation, _: MultiAsset, _: MultiLocation) -> Result<(), LockError> {
		Err(LockError::NotApplicable)
	}
	fn unlock_asset(_: MultiLocation, _: MultiAsset, _: MultiLocation) -> Result<(), LockError> {
		Err(LockError::NotApplicable)
	}
}
