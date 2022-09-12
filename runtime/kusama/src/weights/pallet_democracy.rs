// This file is part of Substrate.

// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{Weight}};
use sp_std::marker::PhantomData;

pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_democracy::WeightInfo for WeightInfo<T> {
	fn propose() -> Weight { Weight::zero() }
	fn second() -> Weight { Weight::zero() }
	fn vote_new() -> Weight { Weight::zero() }
	fn vote_existing() -> Weight { Weight::zero() }
	fn emergency_cancel() -> Weight { Weight::zero() }
	fn blacklist() -> Weight { Weight::zero() }
	fn external_propose() -> Weight { Weight::zero() }
	fn external_propose_majority() -> Weight { Weight::zero() }
	fn external_propose_default() -> Weight { Weight::zero() }
	fn fast_track() -> Weight { Weight::zero() }
	fn veto_external() -> Weight { Weight::zero() }
	fn cancel_proposal() -> Weight { Weight::zero() }
	fn cancel_referendum() -> Weight { Weight::zero() }
	fn on_initialize_base(_r: u32, ) -> Weight { Weight::zero() }
	fn on_initialize_base_with_launch_period(_r: u32, ) -> Weight { Weight::zero() }
	fn delegate(_r: u32, ) -> Weight { Weight::zero() }
	fn undelegate(_r: u32, ) -> Weight { Weight::zero() }
	fn clear_public_proposals() -> Weight { Weight::zero() }
	fn unlock_remove(_r: u32, ) -> Weight { Weight::zero() }
	fn unlock_set(_r: u32, ) -> Weight { Weight::zero() }
	fn remove_vote(_r: u32, ) -> Weight { Weight::zero() }
	fn remove_other_vote(_r: u32, ) -> Weight { Weight::zero() }
}
