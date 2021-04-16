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

use sp_std::result::Result;
use xcm::v0::{Error as XcmError, Result as XcmResult, MultiAsset, MultiLocation};
use crate::Assets;

/// Facility for asset transacting.
///
/// This should work with as many asset/location combinations as possible. Locations to support may include non-
/// account locations such as a `MultiLocation::X1(Junction::Parachain)`. Different chains may handle them in
/// different ways.
pub trait TransactAsset {
	/// Deposit the `what` asset into the account of `who`.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if deposit failed.
	fn deposit_asset(_what: &MultiAsset, _who: &MultiLocation) -> XcmResult {
		Err(XcmError::Unimplemented)
	}

	/// Withdraw the given asset from the consensus system. Return the actual asset(s) withdrawn. In
	/// the case of `what` being a wildcard, this may be something more specific.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if withdraw failed.
	fn withdraw_asset(_what: &MultiAsset, _who: &MultiLocation) -> Result<Assets, XcmError> {
		Err(XcmError::Unimplemented)
	}

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Returns `XcmError::FailedToTransactAsset` if transfer failed.
	fn transfer_asset(_asset: &MultiAsset, _from: &MultiLocation, _to: &MultiLocation) -> Result<Assets, XcmError> {
		Err(XcmError::Unimplemented)
	}

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Attempts to use `transfer_asset` and if not available then falls back to using a two-part withdraw/deposit.
	fn teleport_asset(asset: &MultiAsset, from: &MultiLocation, to: &MultiLocation) -> Result<Assets, XcmError> {
		match Self::transfer_asset(asset, from, to) {
			Err(XcmError::Unimplemented) => {
				let assets = Self::withdraw_asset(asset, from)?;
				// Not a very forgiving attitude; once we implement roll-backs then it'll be nicer.
				Self::deposit_asset(asset, to)?;
				Ok(assets)
			}
			result => result
		}
	}
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl TransactAsset for Tuple {
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		for_tuples!( #(
			match Tuple::deposit_asset(what, who) { o @ Ok(_) => return o, _ => () }
		)* );
		Err(XcmError::Unimplemented)
	}
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<Assets, XcmError> {
		for_tuples!( #(
			match Tuple::withdraw_asset(what, who) { o @ Ok(_) => return o, _ => () }
		)* );
		Err(XcmError::Unimplemented)
	}
	fn transfer_asset(what: &MultiAsset, from: &MultiLocation, to: &MultiLocation) -> Result<Assets, XcmError> {
		for_tuples!( #(
			match Tuple::transfer_asset(what, from, to) { o @ Ok(_) => return o, _ => () }
		)* );
		Err(XcmError::Unimplemented)
	}
}

