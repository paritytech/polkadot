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

use sp_std::result;
use xcm::latest::prelude::*;

pub trait MatchesFungible<Balance> {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<Balance> MatchesFungible<Balance> for Tuple {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance> {
		for_tuples!( #(
			match Tuple::matches_fungible(a) { o @ Some(_) => return o, _ => () }
		)* );
		log::trace!(target: "xcm::matches_fungible", "did not match fungible asset: {:?}", &a);
		None
	}
}

pub trait MatchesNonFungible<Instance> {
	fn matches_nonfungible(a: &MultiAsset) -> Option<Instance>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<Instance> MatchesNonFungible<Instance> for Tuple {
	fn matches_nonfungible(a: &MultiAsset) -> Option<Instance> {
		for_tuples!( #(
			match Tuple::matches_nonfungible(a) { o @ Some(_) => return o, _ => () }
		)* );
		log::trace!(target: "xcm::matches_non_fungible", "did not match non-fungible asset: {:?}", &a);
		None
	}
}

/// Errors associated with [`MatchesFungibles`] operation.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
	/// The given asset is not handled. (According to [`XcmError::AssetNotFound`])
	AssetNotHandled,
	/// `MultiLocation` to `AccountId` conversion failed.
	AccountIdConversionFailed,
	/// `u128` amount to currency `Balance` conversion failed.
	AmountToBalanceConversionFailed,
	/// `MultiLocation` to `AssetId`/`ClassId` conversion failed.
	AssetIdConversionFailed,
	/// `AssetInstance` to non-fungibles instance ID conversion failed.
	InstanceConversionFailed,
}

impl From<Error> for XcmError {
	fn from(e: Error) -> Self {
		use XcmError::FailedToTransactAsset;
		match e {
			Error::AssetNotHandled => XcmError::AssetNotFound,
			Error::AccountIdConversionFailed => FailedToTransactAsset("AccountIdConversionFailed"),
			Error::AmountToBalanceConversionFailed =>
				FailedToTransactAsset("AmountToBalanceConversionFailed"),
			Error::AssetIdConversionFailed => FailedToTransactAsset("AssetIdConversionFailed"),
			Error::InstanceConversionFailed => FailedToTransactAsset("InstanceConversionFailed"),
		}
	}
}

pub trait MatchesFungibles<AssetId, Balance> {
	fn matches_fungibles(a: &MultiAsset) -> result::Result<(AssetId, Balance), Error>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<AssetId, Balance> MatchesFungibles<AssetId, Balance> for Tuple {
	fn matches_fungibles(a: &MultiAsset) -> result::Result<(AssetId, Balance), Error> {
		for_tuples!( #(
			match Tuple::matches_fungibles(a) { o @ Ok(_) => return o, _ => () }
		)* );
		log::trace!(target: "xcm::matches_fungibles", "did not match fungibles asset: {:?}", &a);
		Err(Error::AssetNotHandled)
	}
}

pub trait MatchesNonFungibles<AssetId, Instance> {
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(AssetId, Instance), Error>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<AssetId, Instance> MatchesNonFungibles<AssetId, Instance> for Tuple {
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(AssetId, Instance), Error> {
		for_tuples!( #(
			match Tuple::matches_nonfungibles(a) { o @ Ok(_) => return o, _ => () }
		)* );
		log::trace!(target: "xcm::matches_non_fungibles", "did not match fungibles asset: {:?}", &a);
		Err(Error::AssetNotHandled)
	}
}
