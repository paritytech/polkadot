// This file is part of Substrate.

// Copyright (C) 2019-2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Runtime API definition for pallet-xcm pallet.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Codec, Decode, Encode};
use derivative::Derivative;
use scale_info::TypeInfo;
use sp_runtime::traits::MaybeDisplay;
use xcm::{latest::Error as XcmError, VersionedMultiLocation, VersionedXcm};

// Versioned Weight, to not break api with Weight v2
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum VersionedWeight {
	V2(xcm::v2::Weight),
}

impl From<xcm::v2::Weight> for VersionedWeight {
	fn from(x: xcm::v2::Weight) -> Self {
		VersionedWeight::V2(x)
	}
}

impl TryFrom<VersionedWeight> for xcm::v2::Weight {
	type Error = ();
	fn try_from(x: VersionedWeight) -> Result<Self, ()> {
		match x {
			VersionedWeight::V2(x) => Ok(x),
		}
	}
}

// [`XcmResult`]
pub type XcmResult<T> = Result<T, XcmError>;

sp_api::decl_runtime_apis! {
	pub trait XcmApi<AccountId, RuntimeCall> where
		AccountId: Codec + MaybeDisplay,

	{
		/// Returns weight of a given message
		fn weight_message(message: VersionedXcm<RuntimeCall>) -> XcmResult<VersionedWeight>;

		/// Returns the location to accountId conversion
		fn convert_location(location: VersionedMultiLocation) -> XcmResult<AccountId>;

		/// Returns fee charged for an amount of weight in a concrete multiasset
		fn calculate_concrete_asset_fee(asset_location: VersionedMultiLocation, weight: VersionedWeight) -> XcmResult<u128>;
	}
}
