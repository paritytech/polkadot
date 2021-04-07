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

use sp_std::{prelude::*, result::Result, borrow::Borrow, convert::TryFrom};
use parity_scale_codec::{Encode, Decode};
use xcm::v0::{MultiLocation, OriginKind};

/// Generic third-party conversion trait. Use this when you don't want to force the user to use default
/// impls of `From` and `Into` for the types you wish to convert between.
///
/// One of `convert`/`convert_ref` and `reverse`/`reverse_ref` MUST be implemented. If possible, implement
/// `convert_ref`, since this will never result in a clone. Use `convert` when you definitely need to consume
/// the source value.
pub trait Convert<A: Clone, B: Clone> {
	/// Convert from `value` (of type `A`) into an equivalent value of type `B`, `Err` if not possible.
	fn convert(value: A) -> Result<B, A> { Self::convert_ref(&value).map_err(|_| value) }
	fn convert_ref(value: impl Borrow<A>) -> Result<B, ()> {
		Self::convert(value.borrow().clone()).map_err(|_| ())
	}
	/// Convert from `value` (of type `B`) into an equivalent value of type `A`, `Err` if not possible.
	fn reverse(value: B) -> Result<A, B> { Self::reverse_ref(&value).map_err(|_| value) }
	fn reverse_ref(value: impl Borrow<B>) -> Result<A, ()> {
		Self::reverse(value.borrow().clone()).map_err(|_| ())
	}
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl<A: Clone, B: Clone> Convert<A, B> for Tuple {
	fn convert(value: A) -> Result<B, A> {
		for_tuples!( #(
			let value = match Tuple::convert(value) {
				Ok(result) => return Ok(result),
				Err(v) => v,
			};
		)* );
		Err(value)
	}
	fn reverse(value: B) -> Result<A, B> {
		for_tuples!( #(
			let value = match Tuple::reverse(value) {
				Ok(result) => return Ok(result),
				Err(v) => v,
			};
		)* );
		Err(value)
	}
	fn convert_ref(value: impl Borrow<A>) -> Result<B, ()> {
		let value = value.borrow();
		for_tuples!( #(
			match Tuple::convert_ref(value) {
				Ok(result) => return Ok(result),
				Err(_) => (),
			}
		)* );
		Err(())
	}
	fn reverse_ref(value: impl Borrow<B>) -> Result<A, ()> {
		let value = value.borrow();
		for_tuples!( #(
			match Tuple::reverse_ref(value.clone()) {
				Ok(result) => return Ok(result),
				Err(_) => (),
			}
		)* );
		Err(())
	}
}

/// Simple pass-through which implements `BytesConversion` while not doing any conversion.
pub struct Identity;
impl<T: Clone> Convert<T, T> for Identity {
	fn convert(value: T) -> Result<T, T> { Ok(value) }
	fn reverse(value: T) -> Result<T, T> { Ok(value) }
}

/// Implementation of `Convert` trait using `TryFrom`.
pub struct JustTry;
impl<Source: TryFrom<Dest> + Clone, Dest: TryFrom<Source> + Clone> Convert<Source, Dest> for JustTry {
	fn convert(value: Source) -> Result<Dest, Source> {
		Dest::try_from(value.clone()).map_err(|_| value)
	}
	fn reverse(value: Dest) -> Result<Source, Dest> {
		Source::try_from(value.clone()).map_err(|_| value)
	}
}

/// Implementation of `Convert<_, Vec<u8>>` using the parity scale codec.
pub struct Encoded;
impl<T: Clone + Encode + Decode> Convert<T, Vec<u8>> for Encoded {
	fn convert_ref(value: impl Borrow<T>) -> Result<Vec<u8>, ()> { Ok(value.borrow().encode()) }
	fn reverse_ref(bytes: impl Borrow<Vec<u8>>) -> Result<T, ()> {
		T::decode(&mut &bytes.borrow()[..]).map_err(|_| ())
	}
}

/// Implementation of `Convert<Vec<u8>, _>` using the parity scale codec.
pub struct Decoded;
impl<T: Clone + Encode + Decode> Convert<Vec<u8>, T> for Decoded {
	fn convert_ref(bytes: impl Borrow<Vec<u8>>) -> Result<T, ()> {
		T::decode(&mut &bytes.borrow()[..]).map_err(|_| ())
	}
	fn reverse_ref(value: impl Borrow<T>) -> Result<Vec<u8>, ()> { Ok(value.borrow().encode()) }
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
