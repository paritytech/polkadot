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
impl<T: frame_system::Config> pallet_scheduler::WeightInfo for WeightInfo<T> {
	fn on_initialize_periodic_named_resolved(s: u32, ) -> Weight;
	fn on_initialize_named_resolved(s: u32, ) -> Weight;
	fn on_initialize_periodic_resolved(s: u32, ) -> Weight;
	fn on_initialize_resolved(s: u32, ) -> Weight;
	fn on_initialize_named_aborted(s: u32, ) -> Weight;
	fn on_initialize_aborted(s: u32, ) -> Weight;
	fn on_initialize_periodic_named(s: u32, ) -> Weight;
	fn on_initialize_periodic(s: u32, ) -> Weight;
	fn on_initialize_named(s: u32, ) -> Weight;
	fn on_initialize(s: u32, ) -> Weight;
	fn schedule(s: u32, ) -> Weight;
	fn cancel(s: u32, ) -> Weight;
	fn schedule_named(s: u32, ) -> Weight;
	fn cancel_named(s: u32, ) -> Weight;
}
