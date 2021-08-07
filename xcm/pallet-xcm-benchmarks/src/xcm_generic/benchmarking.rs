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
use crate::{account_id_junction, execute_order, execute_xcm, OverArchingCallOf, XcmCallOf};
use codec::Encode;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_support::{assert_ok, traits::fungible::Inspect as FungibleInspect, weights::Weight};
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

benchmarks! {
	send_xcm {}: {}
	order_noop {
		let order = Order::<XcmCallOf<T>>::Null;
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding = Assets::default();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	}
	xcm_withdraw_asset {}: {} verify {}
	xcm_reserve_asset_deposit {}: {} verify {}
	xcm_teleport_asset {}: {} verify {}
	xcm_transfer_asset {}: {} verify {}
	xcm_transfer_reserve_asset {}: {} verify {}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::xcm_generic::mock::new_test_ext(),
	crate::xcm_generic::mock::Test
);
