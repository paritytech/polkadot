// Copyright 2021 Parity Technologies (UK) Ltd.
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

use crate::*;
use codec::Encode;
use frame_benchmarking::benchmarks;
use frame_support::{
	assert_ok,
	traits::{fungible::Inspect as FungibleInspect, fungibles::Inspect as FungiblesInspect},
	weights::Weight,
};
use sp_runtime::traits::Zero;
use sp_std::{convert::TryInto, prelude::*, vec};
use xcm::{
	opaque::v0::{AssetInstance, ExecuteXcm, Junction, MultiAsset, MultiLocation, NetworkId},
	v0::{Error as XcmError, Order, Outcome, Xcm},
};
use xcm_executor::{traits::TransactAsset, Assets};

// TODO: def. needs to be become a config, might also want to use bounded vec.
const MAX_ASSETS: u32 = 25;

/// The number of fungible assets in the holding.
const HOLDING_FUNGIBLES: u32 = 99;
const HOLDING_NON_FUNGIBLES: u32 = 99;

// Thoughts:
//
// All XCMs should have all of their internals as parameter, regardless of it being used or not.
// This is because some implementations might need them and depend upon them, and some might not.

// Rationale:
//
// Benchmarks ending with _fungible typically indicate the case where there is only one asset in
// the order/xcm, and it is fungible. Typically, an order/xcm that is being weighed with such a
// benchmark will have a `asset: Vec<_>` of length one.
//
// Benchmarks ending with fungibles_per_asset imply that this benchmark is for the case of a chain
// with multiple asset types (thus the name `fungibles`). Such a benchmark will still only work
// with one asset, and it is meant to be summed
benchmarks! {
	where_clause { where
		T::XcmConfig: xcm_executor::Config<Call = OverArchingCallOf<T>>,
		<T::FungiblesTransactAsset as FungiblesInspect<T::AccountId>>::AssetId: From<u32>,
		<
			<
				<T as Config>::FungibleTransactAsset
				as
				FungibleInspect<<T as frame_system::Config>::AccountId>
			>::Balance
			as
			TryInto<u128>
		>::Error: sp_std::fmt::Debug,
		<
			<
				<T as Config>::FungiblesTransactAsset
				as
				FungiblesInspect<<T as frame_system::Config>::AccountId>
			>::Balance
			as
			TryInto<u128>
		>::Error: sp_std::fmt::Debug,
	}

	// a xcm-send operation. This is useful for effects of an order.
	send_xcm {}: {}

	// orders.
	order_null {
		let order = Order::<OverArchingCallOf<T>>::Null;
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding = Assets::default();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	}

	order_deposit_asset {}: {} verify {}
	order_deposit_reserved_asset {}: {} verify {}
	order_exchange_asset {}: {} verify {}
	order_initiate_reserve_withdraw {}: {} verify {}
	order_initiate_teleport {}: {} verify {}
	order_query_holding {}: {} verify {}
	order_buy_execution {}: {} verify {}

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

#[cfg(test)]
mod benchmark_tests {
	use super::*;

	#[test]
	fn order_deposit_asset_fungible() {
		crate::mock_fungible::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungible::Test>(b"")
				.unwrap();
		})
	}
}
