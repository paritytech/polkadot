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
	fn service_agendas_base() -> Weight { Weight::zero() }
	fn service_agenda_base(_s: u32, ) -> Weight { Weight::zero() }
	fn service_task_base() -> Weight { Weight::zero() }
	fn service_task_fetched(_s: u32, ) -> Weight { Weight::zero() }
	fn service_task_named() -> Weight { Weight::zero() }
	fn service_task_periodic() -> Weight { Weight::zero() }
	fn execute_dispatch_signed() -> Weight { Weight::zero() }
	fn execute_dispatch_unsigned() -> Weight { Weight::zero() }
	fn schedule(_s: u32, ) -> Weight { Weight::zero() }
	fn cancel(_s: u32, ) -> Weight { Weight::zero() }
	fn schedule_named(_s: u32, ) -> Weight { Weight::zero() }
	fn cancel_named(_s: u32, ) -> Weight { Weight::zero() }
}
