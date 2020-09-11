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
use xcm::v0::{Error as XcmError, Result as XcmResult, MultiAsset, MultiLocation, MultiOrigin};
use frame_support::traits::Get;

pub trait FilterAssetLocation {
	/// A filter to distinguish between asset/location pairs.
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool;
}

impl FilterAssetLocation for () {
	fn filter_asset_location(_: &MultiAsset, _: &MultiLocation) -> bool {
		false
	}
}

pub struct NativeAsset;
impl FilterAssetLocation for NativeAsset {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		matches!(asset, MultiAsset::ConcreteFungible { ref id, .. } if id == origin)
	}
}

// TODO: Implement for arbitrary tuples.
impl<X: FilterAssetLocation, Y: FilterAssetLocation> FilterAssetLocation for (X, Y) {
	fn filter_asset_location(what: &MultiAsset, origin: &MultiLocation) -> bool {
		X::filter_asset_location(what, origin) || Y::filter_asset_location(what, origin)
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
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult;

	/// Withdraw the given asset from the consensus system. Return the actual asset withdrawn. In
	/// the case of `what` being a wildcard, this may be something more specific.
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<MultiAsset, XcmError>;

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Undefined if from account doesn't own this asset.
	fn transfer_asset(asset: &MultiAsset, from: &MultiLocation, to: &MultiLocation) -> Result<MultiAsset, XcmError> {
		let withdrawn = Self::withdraw_asset(asset, from)?;
		Self::deposit_asset(&withdrawn, to)?;
		Ok(withdrawn)
	}
}

// TODO: Implement for arbitrary tuples.
impl<X: TransactAsset, Y: TransactAsset> TransactAsset for (X, Y) {
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		X::deposit_asset(what, who).or_else(|_| Y::deposit_asset(what, who))
	}
	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<MultiAsset, XcmError> {
		X::withdraw_asset(what, who).or_else(|_| Y::withdraw_asset(what, who))
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
impl<B: From<u128>, X: MatchesFungible<B>, Y: MatchesFungible<B>> MatchesFungible<B> for (X, Y) {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		X::matches_fungible(a).or_else(|| Y::matches_fungible(a))
	}
}

pub trait LocationConversion<AccountId> {
	fn from_location(location: &MultiLocation) -> Option<AccountId>;
	#[deprecated]
	fn into_location(who: AccountId) -> Option<MultiLocation> { Self::try_into_location(who).ok() }
	fn try_into_location(who: AccountId) -> Result<MultiLocation, AccountId>;
}
// TODO: Use tuple generator.
impl<A> LocationConversion<A> for () {
	fn from_location(_: &MultiLocation) -> Option<A> { None }
	fn try_into_location(who: A) -> Result<MultiLocation, A> { Err(who) }
}
impl<A, X: LocationConversion<A>> LocationConversion<A> for (X,) {
	fn from_location(location: &MultiLocation) -> Option<A> {
		X::from_location(location)
	}
	fn try_into_location(who: A) -> Result<MultiLocation, A> {
		X::try_into_location(who)
	}
}
impl<A, X: LocationConversion<A>, Y: LocationConversion<A>> LocationConversion<A> for (X, Y) {
	fn from_location(location: &MultiLocation) -> Option<A> {
		X::from_location(location).or_else(|| Y::from_location(location))
	}
	fn try_into_location(who: A) -> Result<MultiLocation, A> {
		X::try_into_location(who).or_else(|who| Y::try_into_location(who))
	}
}
impl<
	A,
	X: LocationConversion<A>,
	Y: LocationConversion<A>,
	Z: LocationConversion<A>,
> LocationConversion<A> for (X, Y, Z) {
	fn from_location(location: &MultiLocation) -> Option<A> {
		<((X, Y), Z)>::from_location(location)
	}
	fn try_into_location(who: A) -> Result<MultiLocation, A> {
		<((X, Y), Z)>::try_into_location(who)
	}
}
impl<
	A,
	X: LocationConversion<A>,
	Y: LocationConversion<A>,
	Z: LocationConversion<A>,
	W: LocationConversion<A>,
> LocationConversion<A> for (X, Y, Z, W) {
	fn from_location(location: &MultiLocation) -> Option<A> {
		<(((X, Y), Z), W)>::from_location(location)
	}
	fn try_into_location(who: A) -> Result<MultiLocation, A> {
		<(((X, Y), Z), W)>::try_into_location(who)
	}
}

pub trait ConvertOrigin<Origin> {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<Origin, MultiLocation>;
}

// TODO: Use tuple generator.
impl<O> ConvertOrigin<O> for () {
	fn convert_origin(origin: MultiLocation, _: MultiOrigin) -> Result<O, MultiLocation> { Err(origin) }
}

impl<O, X: ConvertOrigin<O>> ConvertOrigin<O> for (X,) {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<O, MultiLocation> {
		X::convert_origin(origin, kind)
	}
}
impl<O, X: ConvertOrigin<O>, Y: ConvertOrigin<O>> ConvertOrigin<O> for (X, Y) {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<O, MultiLocation> {
		X::convert_origin(origin, kind).or_else(|origin| Y::convert_origin(origin, kind))
	}
}
impl<
	O,
	X: ConvertOrigin<O>,
	Y: ConvertOrigin<O>,
	Z: ConvertOrigin<O>,
> ConvertOrigin<O> for (X, Y, Z) {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<O, MultiLocation> {
		<((X, Y), Z)>::convert_origin(origin, kind)
	}
}
impl<
	O,
	X: ConvertOrigin<O>,
	Y: ConvertOrigin<O>,
	Z: ConvertOrigin<O>,
	W: ConvertOrigin<O>,
> ConvertOrigin<O> for (X, Y, Z, W) {
	fn convert_origin(origin: MultiLocation, kind: MultiOrigin) -> Result<O, MultiLocation> {
		<(((X, Y), Z), W)>::convert_origin(origin, kind)
	}
}

pub trait InvertLocation {
	fn invert_location(l: &MultiLocation) -> MultiLocation;
}
