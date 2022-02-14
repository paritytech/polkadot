// Copyright 2020 Parity Technologies (UK) Ltd.
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

use core::{
	convert::{TryFrom, TryInto},
	result::Result,
};
use derivative::Derivative;
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input};
use scale_info::TypeInfo;

pub mod v1;
pub mod v2;
pub mod v3;

pub mod latest {
	pub use super::v3::*;
}

mod double_encoded;
pub use double_encoded::DoubleEncoded;

/// Maximum nesting level for XCM decoding.
pub const MAX_XCM_DECODE_DEPTH: u32 = 8;

/// A version of XCM.
pub type Version = u32;

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

/// A single `MultiLocation` value, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedMultiLocation {
	#[codec(index = 1)]
	V1(v1::MultiLocation),
	#[codec(index = 2)]
	V3(v3::MultiLocation),
}

impl IntoVersion for VersionedMultiLocation {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 | 2 => Self::V1(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl From<v1::MultiLocation> for VersionedMultiLocation {
	fn from(x: v1::MultiLocation) -> Self {
		VersionedMultiLocation::V1(x)
	}
}

impl<T: Into<v3::MultiLocation>> From<T> for VersionedMultiLocation {
	fn from(x: T) -> Self {
		VersionedMultiLocation::V3(x.into())
	}
}

impl TryFrom<VersionedMultiLocation> for v1::MultiLocation {
	type Error = ();
	fn try_from(x: VersionedMultiLocation) -> Result<Self, ()> {
		use VersionedMultiLocation::*;
		match x {
			V1(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl TryFrom<VersionedMultiLocation> for v3::MultiLocation {
	type Error = ();
	fn try_from(x: VersionedMultiLocation) -> Result<Self, ()> {
		use VersionedMultiLocation::*;
		match x {
			V1(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// A single `InteriorMultiLocation` value, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedInteriorMultiLocation {
	#[codec(index = 0)]
	V1(v1::InteriorMultiLocation),
	#[codec(index = 1)]
	V3(v3::InteriorMultiLocation),
}

impl IntoVersion for VersionedInteriorMultiLocation {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 | 2 => Self::V1(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl From<v1::InteriorMultiLocation> for VersionedInteriorMultiLocation {
	fn from(x: v1::InteriorMultiLocation) -> Self {
		VersionedInteriorMultiLocation::V1(x)
	}
}

impl<T: Into<v3::InteriorMultiLocation>> From<T> for VersionedInteriorMultiLocation {
	fn from(x: T) -> Self {
		VersionedInteriorMultiLocation::V3(x.into())
	}
}

impl TryFrom<VersionedInteriorMultiLocation> for v1::InteriorMultiLocation {
	type Error = ();
	fn try_from(x: VersionedInteriorMultiLocation) -> Result<Self, ()> {
		use VersionedInteriorMultiLocation::*;
		match x {
			V1(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl TryFrom<VersionedInteriorMultiLocation> for v3::InteriorMultiLocation {
	type Error = ();
	fn try_from(x: VersionedInteriorMultiLocation) -> Result<Self, ()> {
		use VersionedInteriorMultiLocation::*;
		match x {
			V1(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// A single `Response` value, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedResponse {
	#[codec(index = 1)]
	V1(v1::Response),
	#[codec(index = 2)]
	V2(v2::Response),
	#[codec(index = 3)]
	V3(v3::Response),
}

impl IntoVersion for VersionedResponse {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 => Self::V1(self.try_into()?),
			2 => Self::V2(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl From<v1::Response> for VersionedResponse {
	fn from(x: v1::Response) -> Self {
		VersionedResponse::V1(x)
	}
}

impl From<v2::Response> for VersionedResponse {
	fn from(x: v2::Response) -> Self {
		VersionedResponse::V2(x)
	}
}

impl<T: Into<v3::Response>> From<T> for VersionedResponse {
	fn from(x: T) -> Self {
		VersionedResponse::V3(x.into())
	}
}

impl TryFrom<VersionedResponse> for v1::Response {
	type Error = ();
	fn try_from(x: VersionedResponse) -> Result<Self, ()> {
		use VersionedResponse::*;
		match x {
			V1(x) => Ok(x),
			V2(x) => x.try_into(),
			V3(x) => V2(x.try_into()?).try_into(),
		}
	}
}

impl TryFrom<VersionedResponse> for v2::Response {
	type Error = ();
	fn try_from(x: VersionedResponse) -> Result<Self, ()> {
		use VersionedResponse::*;
		match x {
			V1(x) => x.try_into(),
			V2(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl TryFrom<VersionedResponse> for v3::Response {
	type Error = ();
	fn try_from(x: VersionedResponse) -> Result<Self, ()> {
		use VersionedResponse::*;
		match x {
			V1(x) => V2(x.try_into()?).try_into(),
			V2(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// A single `MultiAsset` value, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedMultiAsset {
	#[codec(index = 1)]
	V1(v1::MultiAsset),
	#[codec(index = 2)]
	V3(v3::MultiAsset),
}

impl IntoVersion for VersionedMultiAsset {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 | 2 => Self::V1(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl From<v1::MultiAsset> for VersionedMultiAsset {
	fn from(x: v1::MultiAsset) -> Self {
		VersionedMultiAsset::V1(x)
	}
}

impl From<v3::MultiAsset> for VersionedMultiAsset {
	fn from(x: v3::MultiAsset) -> Self {
		VersionedMultiAsset::V3(x)
	}
}

impl TryFrom<VersionedMultiAsset> for v1::MultiAsset {
	type Error = ();
	fn try_from(x: VersionedMultiAsset) -> Result<Self, ()> {
		use VersionedMultiAsset::*;
		match x {
			V1(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl TryFrom<VersionedMultiAsset> for v3::MultiAsset {
	type Error = ();
	fn try_from(x: VersionedMultiAsset) -> Result<Self, ()> {
		use VersionedMultiAsset::*;
		match x {
			V1(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// A single `MultiAssets` value, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedMultiAssets {
	#[codec(index = 1)]
	V1(v1::MultiAssets),
	#[codec(index = 2)]
	V3(v3::MultiAssets),
}

impl IntoVersion for VersionedMultiAssets {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 | 2 => Self::V1(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl From<v1::MultiAssets> for VersionedMultiAssets {
	fn from(x: v1::MultiAssets) -> Self {
		VersionedMultiAssets::V1(x)
	}
}

impl<T: Into<v3::MultiAssets>> From<T> for VersionedMultiAssets {
	fn from(x: T) -> Self {
		VersionedMultiAssets::V3(x.into())
	}
}

impl TryFrom<VersionedMultiAssets> for v1::MultiAssets {
	type Error = ();
	fn try_from(x: VersionedMultiAssets) -> Result<Self, ()> {
		use VersionedMultiAssets::*;
		match x {
			V1(x) => Ok(x),
			V3(x) => x.try_into(),
		}
	}
}

impl TryFrom<VersionedMultiAssets> for v3::MultiAssets {
	type Error = ();
	fn try_from(x: VersionedMultiAssets) -> Result<Self, ()> {
		use VersionedMultiAssets::*;
		match x {
			V1(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// A single XCM message, together with its version code.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(Call))]
pub enum VersionedXcm<Call> {
	#[codec(index = 1)]
	V1(v1::Xcm<Call>),
	#[codec(index = 2)]
	V2(v2::Xcm<Call>),
	#[codec(index = 3)]
	V3(v3::Xcm<Call>),
}

impl<C> IntoVersion for VersionedXcm<C> {
	fn into_version(self, n: Version) -> Result<Self, ()> {
		Ok(match n {
			1 => Self::V1(self.try_into()?),
			2 => Self::V2(self.try_into()?),
			3 => Self::V3(self.try_into()?),
			_ => return Err(()),
		})
	}
}

impl<Call> From<v1::Xcm<Call>> for VersionedXcm<Call> {
	fn from(x: v1::Xcm<Call>) -> Self {
		VersionedXcm::V1(x)
	}
}

impl<Call> From<v2::Xcm<Call>> for VersionedXcm<Call> {
	fn from(x: v2::Xcm<Call>) -> Self {
		VersionedXcm::V2(x)
	}
}

impl<Call> From<v3::Xcm<Call>> for VersionedXcm<Call> {
	fn from(x: v3::Xcm<Call>) -> Self {
		VersionedXcm::V3(x)
	}
}

impl<Call> TryFrom<VersionedXcm<Call>> for v1::Xcm<Call> {
	type Error = ();
	fn try_from(x: VersionedXcm<Call>) -> Result<Self, ()> {
		use VersionedXcm::*;
		match x {
			V1(x) => Ok(x),
			V2(x) => x.try_into(),
			V3(x) => V2(x.try_into()?).try_into(),
		}
	}
}

impl<Call> TryFrom<VersionedXcm<Call>> for v2::Xcm<Call> {
	type Error = ();
	fn try_from(x: VersionedXcm<Call>) -> Result<Self, ()> {
		use VersionedXcm::*;
		match x {
			V1(x) => x.try_into(),
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
			V1(x) => V2(x.try_into()?).try_into(),
			V2(x) => x.try_into(),
			V3(x) => Ok(x),
		}
	}
}

/// Convert an `Xcm` datum into a `VersionedXcm`, based on a destination `MultiLocation` which will interpret it.
pub trait WrapVersion {
	fn wrap_version<Call>(
		dest: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<Call>>,
	) -> Result<VersionedXcm<Call>, ()>;
}

/// `()` implementation does nothing with the XCM, just sending with whatever version it was authored as.
impl WrapVersion for () {
	fn wrap_version<Call>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<Call>>,
	) -> Result<VersionedXcm<Call>, ()> {
		Ok(xcm.into())
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to version 2 before wrapping it.
pub struct AlwaysV2;
impl WrapVersion for AlwaysV2 {
	fn wrap_version<Call>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<Call>>,
	) -> Result<VersionedXcm<Call>, ()> {
		Ok(VersionedXcm::<Call>::V2(xcm.into().try_into()?))
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to version 2 before wrapping it.
pub struct AlwaysV3;
impl WrapVersion for AlwaysV3 {
	fn wrap_version<Call>(
		_: &latest::MultiLocation,
		xcm: impl Into<VersionedXcm<Call>>,
	) -> Result<VersionedXcm<Call>, ()> {
		Ok(VersionedXcm::<Call>::V3(xcm.into().try_into()?))
	}
}

/// `WrapVersion` implementation which attempts to always convert the XCM to the latest version before wrapping it.
pub type AlwaysLatest = AlwaysV2;

/// `WrapVersion` implementation which attempts to always convert the XCM to the release version before wrapping it.
pub type AlwaysRelease = AlwaysV2;

pub mod prelude {
	pub use super::{
		latest::prelude::*, AlwaysLatest, AlwaysRelease, AlwaysV2, AlwaysV3, IntoVersion,
		Unsupported, Version as XcmVersion, VersionedInteriorMultiLocation, VersionedMultiAsset,
		VersionedMultiAssets, VersionedMultiLocation, VersionedResponse, VersionedXcm, WrapVersion,
	};
}

pub mod opaque {
	pub mod v1 {
		// Everything from v1
		pub use crate::v1::*;
		// Then override with the opaque types in v1
		pub use crate::v1::opaque::{Order, Xcm};
	}
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
	let _: VersionedMultiAssets = (Here, 1).into();
}
