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
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
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

/// The number of fungible assets in the holding.
const HOLDING_FUNGIBLES: u32 = 99;
const HOLDING_NON_FUNGIBLES: u32 = 99;

fn create_holding(
	fungibles_count: u32,
	fungibles_amount: u128,
	non_fungibles_count: u32,
) -> Assets {
	(0..fungibles_count)
		.map(|i| {
			MultiAsset::ConcreteFungible {
				id: MultiLocation::X1(Junction::GeneralIndex { id: i as u128 }),
				amount: fungibles_amount * i as u128,
			}
			.into()
		})
		.chain((0..non_fungibles_count).map(|i| {
			let bytes = i.encode();
			let mut instance = [0u8; 4];
			instance.copy_from_slice(&bytes);
			MultiAsset::ConcreteNonFungible {
				class: MultiLocation::X1(Junction::GeneralIndex { id: i as u128 }),
				instance: AssetInstance::Array4(instance),
			}
		}))
		.collect::<Vec<_>>()
		.into()
}

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

	order_deposit_asset_fungible_per_asset {
		let amount_balance = T::FungibleTransactAsset::minimum_balance() * 4u32.into();
		let amount: u128 = amount_balance.try_into().unwrap();

		// This is the asset that we want to get from the holding and deposit, which should exist
		// among the assets that `create_holding` is generating
		let asset_multi_location = MultiLocation::Null;
		let asset = MultiAsset::ConcreteFungible { id: MultiLocation::Null , amount };

		let order = Order::<OverArchingCallOf<T>>::DepositAsset {
			assets: vec![asset.clone()],
			dest: MultiLocation::X1(account_id_junction::<T>(77)),
		};
		let origin = MultiLocation::X1(account_id_junction::<T>(1));

		// generate the holding with a bunch of stuff..
		let mut holding = create_holding(HOLDING_FUNGIBLES, amount, HOLDING_NON_FUNGIBLES);
		// .. and the specific asset that we want to take out.
		holding.saturating_subsume(asset);
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		assert_eq!(
			T::FungibleTransactAsset::balance(&account::<T>(77)),
			amount_balance
		)
	}
	order_deposit_asset_fungibles_per_asset {
		// create one asset with our desired id.
		let asset_id = HOLDING_FUNGIBLES / 2;
		let amount_balance = T::FungiblesTransactAsset::minimum_balance(asset_id.into()) * 4u32.into();
		let amount: u128 = amount_balance.try_into().unwrap();
		// assert!(!amount.is_zero(), "amount should never be zero");
		// TODO: we should try and command here the asset to be created, but that's kinda non-trivial.

		let asset = MultiAsset::ConcreteFungible {
			id: MultiLocation::X1(Junction::GeneralIndex { id: 0u128 }),
			amount,
		};
		let order = Order::<OverArchingCallOf<T>>::DepositAsset {
			assets: vec![ asset ],
			dest: MultiLocation::X1(account_id_junction::<T>(77)),
		};
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let holding: Assets = create_holding(HOLDING_FUNGIBLES, amount, HOLDING_NON_FUNGIBLES);
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		assert_eq!(
			T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(77)),
			T::FungiblesTransactAsset::minimum_balance(asset_id.into()) * 4u32.into(),
		)
	}

	order_deposit_reserved_asset_fungible {

	}: {} verify {}
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

	xcm_transfer_reserved_asset_fungible {}: {} verify {}
	xcm_transfer_reserved_asset_fungibles {}: {} verify {}
	xcm_transfer_reserved_asset_fungible_non {}: {} verify {}

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
	fn order_deposit_asset_fungible_per_asset() {
		crate::mock_fungible::new_test_ext().execute_with(|| {
			test_benchmark_order_deposit_asset_fungible_per_asset::<crate::mock_fungible::Test>();
		})
	}

	#[test]
	fn order_deposit_asset_fungibles_per_asset() {
		crate::mock_fungibles::new_test_ext().execute_with(|| {
			test_benchmark_order_deposit_asset_fungibles_per_asset::<crate::mock_fungibles::Test>();
		})
	}

	#[test]
	fn xcm_transfer_asset_fungible() {
		crate::mock_fungible::new_test_ext().execute_with(|| {
			test_benchmark_xcm_transfer_asset_fungible::<crate::mock_fungible::Test>();
		})
	}
	#[test]
	fn xcm_transfer_asset_fungibles() {
		crate::mock_fungibles::new_test_ext().execute_with(|| {
			test_benchmark_xcm_transfer_asset_fungibles::<crate::mock_fungibles::Test>();
		})
	}
}
