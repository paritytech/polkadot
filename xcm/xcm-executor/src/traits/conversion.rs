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
use xcm::v0::{MultiLocation, OriginKind};

// TODO: #2841 #LOCCONV change to use Convert trait.
/// Attempt to convert a location into some value of type `T`, or vice-versa.
pub trait LocationConversion<T> {
	/// Convert `location` into `Some` value of `T`, or `None` if not possible.
	// TODO: #2841 #LOCCONV consider returning Result<T, ()> instead.
	fn from_location(location: &MultiLocation) -> Option<T>;
	/// Convert some value `value` into a `location`, `Err`oring with the original `value` if not possible.
	// TODO: #2841 #LOCCONV consider renaming `into_location`
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
