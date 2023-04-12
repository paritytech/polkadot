// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

// NOTE, this crate is meant to be used in many different environments, notably wasm, but not
// necessarily related to FRAME or even Substrate.
//
// Hence, `no_std` rather than sp-runtime.
#![no_std]
extern crate alloc;

use core::ops::ControlFlow;
use derivative::Derivative;
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input, MaxEncodedLen};
use scale_info::TypeInfo;

pub mod v2;
pub mod v3;

pub mod lts {
	pub use super::v3::*;
}

pub mod latest {
	pub use super::v3::*;
}

mod double_encoded;
pub use double_encoded::DoubleEncoded;

#[cfg(test)]
mod tests;

/// Maximum nesting level for XCM decoding.
pub const MAX_XCM_DECODE_DEPTH: u32 = 8;

/// A version of XCM.
pub type Version = u32;

/// Creates an instruction matcher from an XCM. Since XCM versions differ, we need to make a trait
/// here to unify the interfaces among them.
pub trait CreateMatcher {
	/// The concrete matcher type.
	type Matcher;

	/// Method that creates and returns the matcher type from `Self`.
	fn matcher(self) -> Self::Matcher;
}

/// API that allows to pattern-match against anything that is contained within an XCM.
///
/// The intended usage of the matcher API is to enable the ability to chain successive methods of
/// this trait together, along with the ? operator for the purpose of facilitating the writing,
/// maintenance and auditability of XCM barriers.
///
/// Example:
/// ```rust
/// use xcm::{
/// 	v3::{Instruction, Matcher},
/// 	CreateMatcher, MatchXcm,
/// };
///
/// let mut msg = [Instruction::<()>::ClearOrigin];
/// let res = msg
/// 	.matcher()
/// 	.assert_remaining_insts(1)?
/// 	.match_next_inst(|inst| match inst {
/// 		Instruction::<()>::ClearOrigin => Ok(()),
/// 		_ => Err(()),
/// 	});
/// assert!(res.is_ok());
///
/// Ok::<(), ()>(())
/// ```
pub trait MatchXcm {
	/// The concrete instruction type. Necessary to specify as it changes between XCM versions.
	type Inst;
	/// The `MultiLocation` type. Necessary to specify as it changes between XCM versions.
	type Loc;
	/// The error type to throw when errors happen during matching.
	type Error;

	/// Returns success if the number of instructions that still have not been iterated over
	/// equals `n`, otherwise returns an error.
	fn assert_remaining_insts(self, n: usize) -> Result<Self, Self::Error>
	where
		Self: Sized;

	/// Accepts a closure `f` that contains an argument signifying the next instruction to be
	/// iterated over. The closure can then be used to check whether the instruction matches a
	/// given condition, and can also be used to mutate the fields of an instruction.
	///
	/// The closure `f` returns success when the instruction passes the condition, otherwise it
	/// returns an error, which will ultimately be returned by this function.
	fn match_next_inst<F>(self, f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		F: FnMut(&mut Self::Inst) -> Result<(), Self::Error>;

	/// Attempts to continuously iterate through the instructions while applying `f` to each of
	/// them, until either the last instruction or `cond` returns false.
	///
	/// If `f` returns an error, then iteration halts and the function returns that error.
	/// Otherwise, `f` returns a `ControlFlow` which signifies whether the iteration breaks or
	/// continues.
	fn match_next_inst_while<C, F>(self, cond: C, f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
		F: FnMut(&mut Self::Inst) -> Result<ControlFlow<()>, Self::Error>;

	/// Iterate instructions forward until `cond` returns false. When there are no more instructions
	/// to be read, an error is returned.
	fn skip_inst_while<C>(self, cond: C) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
	{
		Self::match_next_inst_while(self, cond, |_| Ok(ControlFlow::Continue(())))
	}
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Unsupported {}
impl Encode for Unsupported {}
impl Decode for Unsupported {
	fn decode<I: Input>(_: &mut I) -> Result<Self, CodecError> {
		Err("Not decodable".into())
	}
}

/// Attempt to convert `self` into a particular version of itself.
pub trait IntoVersion: Sized {
	/// Consume `self` and return same value expressed in some particular `version` of XCM.
	fn into_version(self, version: Version) -> Result<Self, ()>;

	/// Consume `self` and return same value expressed the latest version of XCM.
	fn into_latest(self) -> Result<Self, ()> {
		self.into_version(latest::VERSION)
	}
}

pub trait TryAs<T> {
	fn try_as(&self) -> Result<&T, ()>;
}

macro_rules! versioned_type {
	($(#[$attr:meta])* pub enum $n:ident {
		$(#[$index3:meta])+
		V3($v3:ty),
	}) => {
		#[derive(Derivative, Encode, Decode, TypeInfo)]
		#[derivative(
			Clone(bound = ""),
			Eq(bound = ""),
			PartialEq(bound = ""),
			Debug(bound = "")
		)]
		#[codec(encode_bound())]
		#[codec(decode_bound())]
		$(#[$attr])*
		pub enum $n {
			$(#[$index3])*
			V3($v3),
		}
		impl $n {
			pub fn try_as<T>(&self) -> Result<&T, ()> where Self: TryAs<T> {
				<Self as TryAs<T>>::try_as(&self)
			}
		}
		impl TryAs<$v3> for $n {
			fn try_as(&self) -> Result<&$v3, ()> {
				match &self {
					Self::V3(ref x) => Ok(x),
				}
			}
		}
		impl IntoVersion for $n {
			fn into_version(self, n: Version) -> Result<Self, ()> {
				Ok(match n {
					3 => Self::V3(self.try_into()?),
					_ => return Err(()),
				})
			}
		}
		impl<T: Into<$v3>> From<T> for $n {
			fn from(x: T) -> Self {
				$n::V3(x.into())
			}
		}
		impl TryFrom<$n> for $v3 {
			type Error = ();
			fn try_from(x: $n) -> Result<Self, ()> {
				use $n::*;
				match x {
					V3(x) => Ok(x),
				}
			}
		}
		impl MaxEncodedLen for $n {
			fn max_encoded_len() -> usize {
				<$v3>::max_encoded_len()
			}
		}
	};

	($(#[$attr:meta])* pub enum $n:ident {
		$(#[$index2:meta])+
		V2($v2:ty),
		$(#[$index3:meta])+
		V3($v3:ty),
	}) => {
		#[derive(Derivative, Encode, Decode, TypeInfo)]
		#[derivative(
			Clone(bound = ""),
			Eq(bound = ""),
			PartialEq(bound = ""),
			Debug(bound = "")
		)]
		#[codec(encode_bound())]
		#[codec(decode_bound())]
		$(#[$attr])*
		pub enum $n {
			$(#[$index2])*
			V2($v2),
			$(#[$index3])*
			V3($v3),
		}
		impl $n {
			pub fn try_as<T>(&self) -> Result<&T, ()> where Self: TryAs<T> {
				<Self as TryAs<T>>::try_as(&self)
			}
		}
		impl TryAs<$v2> for $n {
			fn try_as(&self) -> Result<&$v2, ()> {
				match &self {
					Self::V2(ref x) => Ok(x),
					_ => Err(()),
				}
			}
		}
		impl TryAs<$v3> for $n {
			fn try_as(&self) -> Result<&$v3, ()> {
				match &self {
					Self::V3(ref x) => Ok(x),
					_ => Err(()),
				}
			}
		}
		impl IntoVersion for $n {
			fn into_version(self, n: Version) -> Result<Self, ()> {
				Ok(match n {
					1 | 2 => Self::V2(self.try_into()?),
					3 => Self::V3(self.try_into()?),
					_ => return Err(()),
				})
			}
		}
		impl From<$v2> for $n {
			fn from(x: $v2) -> Self {
				$n::V2(x)
			}
		}
		impl<T: Into<$v3>> From<T> for $n {
			fn from(x: T) -> Self {
				$n::V3(x.into())
			}
		}
		impl TryFrom<$n> for $v2 {
			type Error = ();
			fn try_from(x: $n) -> Result<Self, ()> {
				use $n::*;
				match x {
					V2(x) => Ok(x),
					V3(x) => x.try_into(),
				}
			}
		}
		impl TryFrom<$n> for $v3 {
			type Error = ();
			fn try_from(x: $n) -> Result<Self, ()> {
				use $n::*;
				match x {
					V2(x) => x.try_into(),
					V3(x) => Ok(x),
				}
			}
		}
		impl MaxEncodedLen for $n {
			fn max_encoded_len() -> usize {
				<$v3>::max_encoded_len()
			}
		}
	};
}

versioned_type! {
	/// A single version's `Response` value, together with its version code.
	pub enum VersionedAssetId {
		#[codec(index = 3)]
		V3(v3::AssetId),
	}
}

versioned_type! {
	/// A single version's `Response` value, together with its version code.
	pub enum VersionedResponse {
		#[codec(index = 2)]
		V2(v2::Response),
		#[codec(index = 3)]
		V3(v3::Response),
	}
}

versioned_type! {
	/// A single `MultiLocation` value, together with its version code.
	#[derive(Ord, PartialOrd)]
	pub enum VersionedMultiLocation {
		#[codec(index = 1)] // v2 is same as v1 and therefore re-using the v1 index
		V2(v2::MultiLocation),
		#[codec(index = 3)]
		V3(v3::MultiLocation),
	}
}

versioned_type! {
	/// A single `InteriorMultiLocation` value, together with its version code.
	pub enum VersionedInteriorMultiLocation {
		#[codec(index = 2)] // while this is same as v1::Junctions, VersionedInteriorMultiLocation is introduced in v3
		V2(v2::InteriorMultiLocation),
		#[codec(index = 3)]
		V3(v3::InteriorMultiLocation),
	}
}

versioned_type! {
	/// A single `MultiAsset` value, together with its version code.
	pub enum VersionedMultiAsset {
		#[codec(index = 1)] // v2 is same as v1 and therefore re-using the v1 index
		V2(v2::MultiAsset),
		#[codec(index = 3)]
		V3(v3::MultiAsset),
	}
}

versioned_type! {
	/// A single `MultiAssets` value, together with its version code.
	pub enum VersionedMultiAssets {
		#[codec(index = 1)] // v2 is same as v1 and therefore re-using the v1 index
		V2(v2::MultiAssets),
		#[codec(index = 3)]
		V3(v3::MultiAssets),
	}
}

/// A single XCM message, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(RuntimeCall))]
pub enum VersionedXcm<RuntimeCall> {
	#[codec(index = 2)]
	V2(v2::Xcm<RuntimeCall>),
	#[codec(index = 3)]
	V3(v3::Xcm<RuntimeCall>),
}

impl<C> IntoVersion for VersionedXcm<C> {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			2 => Self::V2(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl<RuntimeCall> From<v2::Xcm<RuntimeCall>> for VersionedXcm<RuntimeCall> {
	fn from(x: v2::Xcm<RuntimeCall>) -> Self {
		VersionedXcm::V2(x)
	}
}

impl<RuntimeCall> From<v3::Xcm<RuntimeCall>> for VersionedXcm<RuntimeCall> {
	fn from(x: v3::Xcm<RuntimeCall>) -> Self {
		VersionedXcm::V3(x)
	}
}

impl<RuntimeCall> TryFrom<VersionedXcm<RuntimeCall>> for v2::Xcm<RuntimeCall> {
	type Error = ();
	fn try_from(x: VersionedXcm<RuntimeCall>) -> Result<Self, ()> {
		use VersionedXcm::*;
		match x {
			V2(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl<Call> TryFrom<VersionedXcm<Call>> for v3::Xcm<Call> {
	type Error = ();
	fn try_from(x: VersionedXcm<Call>) -> Result<Self, ()> {
		use VersionedXcm::*;
		match x {
			V2(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// Convert an `Xcm` datum into a `VersionedXcm`, based on a destination `MultiLocation` which will interpret it.
pub trait WrapVersion {
	fn wrap_version<RuntimeCall>(
		dest: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<RuntimeCall>>,
	) -> Result<VersionedXcm<RuntimeCall>, ()>;
}

/// `()` implementation does nothing with the XCM, just sending with whatever version it was authored as.
impl WrapVersion for () {
	fn wrap_version<RuntimeCall>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<RuntimeCall>>,
	) -> Result<VersionedXcm<RuntimeCall>, ()> {
		Ok(xcm.into())
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to version 2 before wrapping it.
pub struct AlwaysV2;
impl WrapVersion for AlwaysV2 {
	fn wrap_version<RuntimeCall>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<RuntimeCall>>,
	) -> Result<VersionedXcm<RuntimeCall>, ()> {
		Ok(VersionedXcm::<RuntimeCall>::V2(xcm.into().try_into()?))
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to version 3 before wrapping it.
pub struct AlwaysV3;
impl WrapVersion for AlwaysV3 {
	fn wrap_version<Call>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<Call>>,
	) -> Result<VersionedXcm<Call>, ()> {
		Ok(VersionedXcm::<Call>::V3(xcm.into().try_into()?))
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to the latest version
/// before wrapping it.
pub type AlwaysLatest = AlwaysV3;

/// `WrapVersion` implementation which attempts to always convert the XCM to the most recent Long-
/// Term-Support version before wrapping it.
pub type AlwaysLts = AlwaysV3;

pub mod prelude {
	pub use super::{
		latest::prelude::*, AlwaysLatest, AlwaysLts, AlwaysV2, AlwaysV3, IntoVersion, Unsupported,
		Version as XcmVersion, VersionedAssetId, VersionedInteriorMultiLocation,
		VersionedMultiAsset, VersionedMultiAssets, VersionedMultiLocation, VersionedResponse,
		VersionedXcm, WrapVersion,
	};
}

pub mod opaque {
	pub mod v2 {
		// Everything from v2
		pub use crate::v2::*;
		// Then override with the opaque types in v2
		pub use crate::v2::opaque::{Instruction, Xcm};
	}
	pub mod v3 {
		// Everything from v3
		pub use crate::v3::*;
		// Then override with the opaque types in v3
		pub use crate::v3::opaque::{Instruction, Xcm};
	}

	pub mod latest {
		pub use super::v3::*;
	}

	pub mod lts {
		pub use super::v3::*;
	}

	/// The basic `VersionedXcm` type which just uses the `Vec<u8>` as an encoded call.
	pub type VersionedXcm = super::VersionedXcm<()>;
}

// A simple trait to get the weight of some object.
pub trait GetWeight<W> {
	fn weight(&self) -> latest::Weight;
}

#[test]
fn conversion_works() {
	use latest::prelude::*;
	let _: VersionedMultiAssets = (Here, 1u128).into();
}
