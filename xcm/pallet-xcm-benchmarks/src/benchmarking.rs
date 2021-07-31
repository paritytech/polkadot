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

use crate::*;
use codec::Encode;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_support::{assert_ok, traits::{fungible::Inspect as FungibleInspect, fungibles::Inspect as FungiblesInspect}, weights::Weight};
use sp_std::{vec, convert::TryInto, prelude::*};
use xcm::{
	opaque::v0::{ExecuteXcm, Junction, MultiAsset, MultiLocation, NetworkId},
	v0::{Error as XcmError, Order, Outcome, Xcm},
};
use xcm_executor::{traits::TransactAsset, Assets};

/// The xcm executor to use for doing stuff.
pub type ExecutorOf<T> = xcm_executor::XcmExecutor<<T as crate::Config>::XcmConfig>;
/// The asset transactor of our executor
pub type AssetTransactorOf<T> = <<T as Config>::XcmConfig as xcm_executor::Config>::AssetTransactor;
/// The overarching call type.
pub type OverArchingCallOf<T> = <T as frame_system::Config>::Call;
/// The call type of executor's config. Should eventually resolve to the same overarching call type.
pub type XcmCallOf<T> = <<T as Config>::XcmConfig as xcm_executor::Config>::Call;

// TODO: def. needs to be become a config, might also want to use bounded vec.
const MAX_ASSETS: u32 = 25;
const SEED: u32 = 0;

/// wrapper to execute single order. Can be any hack, for now we just do a noop-xcm with a single
/// order.
fn execute_order<T: Config>(
	origin: MultiLocation,
	mut holding: Assets,
	order: Order<XcmCallOf<T>>,
) -> Result<Weight, XcmError> {
	ExecutorOf::<T>::do_execute_effects(&origin, &mut holding, order)
}

/// Execute an xcm.
fn execute_xcm<T: Config>(origin: MultiLocation, xcm: Xcm<XcmCallOf<T>>) -> Outcome {
	ExecutorOf::<T>::execute_xcm(origin, xcm, 999_999) // TODO: what should be the weight be?
}

fn account<T: Config>(index: u32) -> T::AccountId {
	frame_benchmarking::account::<T::AccountId>("account", index, SEED)
}

/// Build a multi-location from an account id.
fn account_id_junction<T: Config>(index: u32) -> Junction {
	let account = account::<T>(index);
	let mut encoded = account.encode();
	encoded.resize(32, 0u8);
	let mut id = [0u8; 32];
	id.copy_from_slice(&encoded);
	Junction::AccountId32 { network: NetworkId::Any, id }
}

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

	order_deposit_asset_fungible {
		let amount_balance = T::FungibleTransactAsset::minimum_balance() * 4u32.into();
		let amount: u128 = amount_balance.try_into().unwrap();

		let asset = MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: amount };

		let order = Order::<OverArchingCallOf<T>>::DepositAsset {
			assets: vec![asset],
			dest: MultiLocation::X1(account_id_junction::<T>(77)),
		};
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding: Assets = MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount }.into();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		assert_eq!(
			T::FungibleTransactAsset::balance(&account::<T>(77)),
			amount_balance
		)
	}
	order_deposit_asset_fungibles {
		let a in 1..MAX_ASSETS;

		let assets = (0..a).map(|_| {
			let amount_balance = T::FungiblesTransactAsset::minimum_balance(a.into()) * 4u32.into();
			let amount: u128 = amount_balance.try_into().unwrap();
			MultiAsset::ConcreteFungible {
				id: MultiLocation::X1(Junction::GeneralIndex { id: a as u128 }),
				amount,
			}
		}).collect::<Vec<_>>();

		let order = Order::<OverArchingCallOf<T>>::DepositAsset {
			assets: assets.clone(),
			dest: MultiLocation::X1(account_id_junction::<T>(77)),
		};
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding: Assets = assets.into();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		for asset_id in 0..a {
			assert_eq!(
				T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(77)),
				T::FungiblesTransactAsset::minimum_balance(asset_id.into()) * 4u32.into(),
			)
		}
	}

	order_deposit_reserved_asset_fungible {}: {} verify {}
	order_deposit_reserved_asset_fungibles {}: {} verify {}

	order_exchange_asset_fungible {}: {} verify {}
	order_exchange_asset_fungibles {}: {} verify {}

	order_initiate_reserve_withdraw_fungible {}: {} verify {}
	order_initiate_reserve_withdraw_fungibles {}: {} verify {}

	order_initiate_teleport_fungible {}: {} verify {}
	order_initiate_teleport_fungibles {}: {} verify {}

	order_query_holding_fungible {}: {} verify {}
	order_query_holding_fungibles {}: {} verify {}

	order_buy_execution_fungible {}: {} verify {}
	order_buy_execution_fungibles {}: {} verify {}

	// base XCM messages.
	xcm_withdraw_asset {}: {} verify {}
	xcm_reserve_asset_deposit {}: {} verify {}
	xcm_teleport_asset {}: {} verify {}
	xcm_query_response {}: {} verify {}
	xcm_transfer_asset_fungible {
		let amount_balance = T::FungibleTransactAsset::minimum_balance() * 4u32.into();
		let amount: u128 = amount_balance.try_into().unwrap();

		let origin: MultiLocation = (account_id_junction::<T>(1)).into();
		let asset = MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount };
		<AssetTransactorOf<T>>::deposit_asset(&asset, &origin).unwrap();

		let dest = (account_id_junction::<T>(2)).into();
		let assets = vec![ asset ];
		let xcm = Xcm::TransferAsset { assets, dest };
	}: {
		assert_ok!(execute_xcm::<T>(origin, xcm).ensure_complete());
	} verify {
		assert_eq!(
			T::FungibleTransactAsset::balance(&account::<T>(2)),
			amount_balance,
		)
	}
	xcm_transfer_asset_fungibles {
		let a in 1..MAX_ASSETS;
		// TODO: incorporate Gav's note on the fact that the type of asset in the holding will affect the worse
		// case.
		let origin: MultiLocation = (account_id_junction::<T>(1)).into();

		let assets = (0..a).map(|_| {
			let amount_balance = T::FungiblesTransactAsset::minimum_balance(a.into()) * 4u32.into();
			let amount: u128 = amount_balance.try_into().unwrap();
			MultiAsset::ConcreteFungible {
				id: MultiLocation::X1(Junction::GeneralIndex { id: a as u128 }),
				amount,
			}
		}).collect::<Vec<_>>();

		let dest = (account_id_junction::<T>(2)).into();
		let xcm = Xcm::TransferAsset { assets, dest };
	}: {
		assert_ok!(execute_xcm::<T>(origin, xcm).ensure_complete());
	} verify {
		for asset_id in 0..a {
			assert_eq!(
				T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(2)),
				T::FungiblesTransactAsset::minimum_balance(asset_id.into()) * 4u32.into(),
			)
		}
	}
	xcm_transfer_reserved_asset {}: {} verify {}
	xcm_transact {}: {} verify {}
	xcm_hrmp_channel_open_request {}: {} verify {}
	xcm_hrmp_channel_accepted {}: {} verify {}
	xcm_hrmp_channel_closing {}: {} verify {}
	xcm_relayed_from {}: {} verify {}
}

impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
