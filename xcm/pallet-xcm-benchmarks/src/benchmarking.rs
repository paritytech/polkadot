// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
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

#![cfg(feature = "runtime-benchmarks")]

use crate::*;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use xcm::{
	opaque::v0::{ExecuteXcm, MultiLocation},
	v0::Order,
};

/// The xcm executor to use for doing stuff.
pub type ExecutorOf<T> = xcm_executor::XcmExecutor<<T as crate::Config>::XcmConfig>;
// TODO: might be better to just accept the outer call directly.
pub type OverArchingCallOf<T> = <T as frame_system::Config>::Call;

benchmarks! {
	where_clause {
		where T::XcmConfig: xcm_executor::Config<Call = OverArchingCallOf<T>>
	}
	// orders.
	order_null {}: {} verify {}
	order_deposit_asset {
		let order = Order::<OverArchingCallOf<T>>::DepositAsset {
			assets: vec![],
			dest: MultiLocation::Null,
		};
	}: {
		ExecutorOf::<T>::execute_order(order)
	} verify {}

	order_deposit_reserved_asset {}: {} verify {}
	order_exchange_asset {}: {} verify {}
	order_initiate_reserve_withdraw {}: {} verify {}
	order_initiate_teleport {}: {} verify {}
	order_query_holding {}: {} verify {}
	order_buy_execution {}: {} verify {}

	// base XCM messages.
	xcm_withdraw_asset {}: {} verify {}
	xcm_reserve_asset_deposit {}: {} verify {}
	xcm_teleport_asset {}: {} verify {}
	xcm_query_response {}: {} verify {}
	xcm_transfer_asset {}: {} verify {}
	xcm_transfer_reserved_asset {}: {} verify {}
	xcm_transact {}: {} verify {}
	xcm_hrmp_channel_open_request {}: {} verify {}
	xcm_hrmp_channel_accepted {}: {} verify {}
	xcm_hrmp_channel_closing {}: {} verify {}
	xcm_relayed_from {}: {} verify {}
}

impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
