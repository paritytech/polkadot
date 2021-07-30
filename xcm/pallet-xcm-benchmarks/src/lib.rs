// This file is part of Substrate.

// Copyright (C) 2017-2021 Parity Technologies (UK) Ltd.
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

//! Pallet that serves no other purpose than benchmarking raw messages [`Xcm`].

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::prelude::*;
pub use pallet::*;

#[cfg(test)]
mod mock;

mod benchmarking;
// pub mod weights;
// pub use weights::*;

#[frame_support::pallet]
pub mod pallet {
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The XCM configurations.
		///
		/// These might affect the execution of XCM messages, such as defining how the
		/// `TransactAsset` is implemented.
		type XcmConfig: xcm_executor::Config;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

}
