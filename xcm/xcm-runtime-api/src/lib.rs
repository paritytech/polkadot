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

use codec::Codec;
use sp_runtime::traits::MaybeDisplay;
use xcm::{
	latest::{Error as XcmError, Weight},
	VersionedMultiLocation, VersionedXcm,
};
// [`XcmResult`]
pub type XcmResult<T> = Result<T, XcmError>;

sp_api::decl_runtime_apis! {
	pub trait PalletXcmApi<AccountId, RuntimeCall> where
		AccountId: Codec + MaybeDisplay,

	{
		/// Returns weight of a given message
		fn weight_message(message: VersionedXcm<RuntimeCall>) -> XcmResult<Weight>;

		/// Returns the location to accountId conversion
		fn convert_location(location: VersionedMultiLocation) -> XcmResult<AccountId>;

		/// Returns fee charged for an amount of weight in a concrete multiasset
		fn calculate_concrete_asset_fee(asset_location: VersionedMultiLocation, weight: Weight) -> XcmResult<u128>;
	}
}
