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

use sp_std::{result::Result, marker::PhantomData, convert::TryFrom};
use sp_runtime::traits::CheckedConversion;
use xcm::v0::{Error as XcmError, Result as XcmResult, MultiAsset, MultiLocation, Junction, OriginKind};
use frame_support::traits::Get;
use super::Assets;

pub trait FilterAssetLocation {
	/// A filter to distinguish between asset/location pairs.
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl FilterAssetLocation for Tuple {
	fn filter_asset_location(what: &MultiAsset, origin: &MultiLocation) -> bool {
		for_tuples!( #(
			if Tuple::filter_asset_location(what, origin) { return true }
		)* );
		false
	}
}

pub struct NativeAsset;
impl FilterAssetLocation for NativeAsset {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		matches!(asset, MultiAsset::ConcreteFungible { ref id, .. } if id == origin)
	}
}


pub struct Case<T>(PhantomData<T>);
impl<T: Get<(MultiAsset, MultiLocation)>> FilterAssetLocation for Case<T> {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		let (a, o) = T::get();
		&a == asset && &o == origin
	}
}

/// Facility for asset transacting.
///
/// This should work with as many asset/location combinations as possible. Locations to support may include non-
/// account locations such as a `MultiLocation::X1(Junction::Parachain)`. Different chains may handle them in
/// different ways.
pub trait TransactAsset {
	/// Deposit the `what` asset into the account of `who`.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if deposit failed.
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult;

	/// Withdraw the given asset from the consensus system. Return the actual asset(s) withdrawn. In
	/// the case of `what` being a wildcard, this may be something more specific.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if withdraw failed.
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<Assets, XcmError>;

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Returns `XcmError::FailedToTransactAsset` if transfer failed.
	fn transfer_asset(asset: &MultiAsset, from: &MultiLocation, to: &MultiLocation) -> Result<Assets, XcmError> {
		let withdrawn = Self::withdraw_asset(asset, from)?;
		for asset in withdrawn.assets_iter() {
			Self::deposit_asset(&asset, to)?;
		}
		Ok(withdrawn)
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
}


pub trait MatchesFungible<Balance> {
	fn matches_fungible(a: &MultiAsset) -> Option<Balance>;
}
pub struct IsConcrete<T>(PhantomData<T>);
impl<T: Get<MultiLocation>, B: TryFrom<u128>> MatchesFungible<B> for IsConcrete<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::ConcreteFungible { id, amount } if id == &T::get() =>
				CheckedConversion::checked_from(*amount),
			_ => None,
		}
	}
}
pub struct IsAbstract<T>(PhantomData<T>);
impl<T: Get<&'static [u8]>, B: TryFrom<u128>> MatchesFungible<B> for IsAbstract<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::AbstractFungible { id, amount } if &id[..] == T::get() =>
				CheckedConversion::checked_from(*amount),
			_ => None,
		}
	}
}
// TODO: impl for tuples
impl<B: From<u128>, X: MatchesFungible<B>, Y: MatchesFungible<B>> MatchesFungible<B> for (X, Y) {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		X::matches_fungible(a).or_else(|| Y::matches_fungible(a))
	}
}

// TODO: change to use Convert trait.
/// Attempt to convert a location into some value of type `T`, or vice-versa.
pub trait LocationConversion<T> {
	/// Convert `location` into `Some` value of `T`, or `None` if not possible.
	// TODO: consider returning Result<T, ()> instead.
	fn from_location(location: &MultiLocation) -> Option<T>;
	/// Convert some value `value` into a `location`, `Err`oring with the original `value` if not possible.
	// TODO: consider renaming `into_location`
	fn try_into_location(value: T) -> Result<MultiLocation, T>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<AccountId> LocationConversion<AccountId> for Tuple {
	fn from_location(location: &MultiLocation) -> Option<AccountId> {
		for_tuples!( #(
			if let Some(result) = Tuple::from_location(location) { return Some(result) }
		)* );
		None
	}
	fn try_into_location(who: AccountId) -> Result<MultiLocation, AccountId> {
		for_tuples!( #(
			let who = match Tuple::try_into_location(who) { Err(w) => w, r => return r };
		)* );
		Err(who)
	}
}

pub trait ConvertOrigin<Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<O> ConvertOrigin<O> for Tuple {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<O, MultiLocation> {
		for_tuples!( #(
			let origin = match Tuple::convert_origin(origin, kind) { Err(o) => o, r => return r };
		)* );
		Err(origin)
	}
}

/// Means of inverting a location: given a location which describes a `target` interpreted from the `source`, this
/// will provide the corresponding location which describes the `source`
pub trait InvertLocation {
	fn invert_location(l: &MultiLocation) -> MultiLocation;
}

/// Simple location inverter; give it this location's ancestry and it'll figure out the inverted location.
pub struct LocationInverter<Ancestry>(PhantomData<Ancestry>);
impl<Ancestry: Get<MultiLocation>> InvertLocation for LocationInverter<Ancestry> {
	fn invert_location(location: &MultiLocation) -> MultiLocation {
		let mut ancestry = Ancestry::get();
		let mut result = location.clone();
		for (i, j) in location.iter_rev()
			.map(|j| match j {
				Junction::Parent => ancestry.take_first().unwrap_or(Junction::OnlyChild),
				_ => Junction::Parent,
			})
			.enumerate()
		{
			*result.at_mut(i).expect("location and result begin equal; same size; qed") = j;
		}
		result
	}
}

