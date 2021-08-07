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

use super::*;
use crate::{
	account_id_junction, execute_order, execute_xcm, worst_case_holding, OverArchingCallOf,
	XcmCallOf,
};
use codec::Encode;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_support::{
	assert_ok, pallet_prelude::Get, traits::fungible::Inspect as FungibleInspect, weights::Weight,
};
use sp_runtime::traits::Zero;
use sp_std::{convert::TryInto, prelude::*, vec};
use xcm::latest::{
	AssetId::*,
	AssetInstance, Error as XcmError, ExecuteXcm, Junction,
	Junction::*,
	MultiAsset,
	MultiAssetFilter::*,
	MultiLocation::{self, *},
	NetworkId, Order, Outcome, WildMultiAsset, Xcm,
};
use xcm_executor::{traits::TransactAsset, Assets};

benchmarks! {
	order_noop {
		let order = Order::<XcmCallOf<T>>::Noop;
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding = worst_case_holding();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	}

	order_query_holding {
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding = worst_case_holding();

		let order = Order::<XcmCallOf<T>>::QueryHolding {
			query_id: Default::default(),
			dest: T::ValidDestination::get(),
			assets: Wild(WildMultiAsset::All), // TODO is worst case filter?
		};

	} : {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		// The assert above is enough to validate this is completed.
		// todo maybe XCM sender peek
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	order_buy_execution {
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding = worst_case_holding();

		let fee_asset = Concrete(Here);

		let order = Order::<XcmCallOf<T>>::BuyExecution {
			fees: (fee_asset, 100_000_000).into(), // should be something inside of holding
			weight: 100_000_000, // TODO think about sensible numbers
			debt: 100_000_000, // TODO think about sensible numbers
			halt_on_error: false,
			orders: Default::default(), // no orders
			instructions: Default::default(), // no instructions
		};
	} : {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {

	}
	xcm_reserve_asset_deposited {}: {} verify {}
	xcm_query_response {}: {} verify {}
	xcm_transact {}: {} verify {}
	xcm_hrmp_new_channel_open_request {}: {} verify {}
	xcm_hrmp_channel_accepted {}: {} verify {}
	xcm_hrmp_channel_closing {}: {} verify {}
	xcm_relayed_from {}: {} verify {}

}

impl_benchmark_test_suite!(
	Pallet,
	crate::xcm_generic::mock::new_test_ext(),
	crate::xcm_generic::mock::Test
);
