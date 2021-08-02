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
		.chain((0..non_fungibles_count).map(|i| MultiAsset::ConcreteNonFungible {
			class: MultiLocation::X1(Junction::GeneralIndex { id: i as u128 }),
			instance: asset_instance_from(i),
		}))
		.collect::<Vec<_>>()
		.into()
}

fn asset_instance_from(x: u32) -> AssetInstance {
	let bytes = x.encode();
	let mut instance = [0u8; 4];
	instance.copy_from_slice(&bytes);
	AssetInstance::Array4(instance)
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
	ExecutorOf::<T>::execute_xcm(origin, xcm, 999_999_999_999) // TODO: very large weight to ensure all benchmarks execute, sensible?
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

	order_deposit_asset_fungible {
		let origin = MultiLocation::X1(account_id_junction::<T>(1));
		let (holding, order) = if let Some((asset, amount)) = T::fungible_asset(1) {
			let order = Order::<OverArchingCallOf<T>>::DepositAsset {
				assets: vec![asset.clone()],
				dest: MultiLocation::X1(account_id_junction::<T>(77)),
			};

			// generate the holding with a bunch of stuff..
			let mut holding = create_holding(HOLDING_FUNGIBLES, amount, HOLDING_NON_FUNGIBLES);
			// .. and the specific asset that we want to take out.
			holding.saturating_subsume(asset);
			assert!(T::FungibleTransactAsset::balance(&account::<T>(77)).is_zero());

			(holding, order)
		} else {
			// just put some mock values in there.
			(Default::default(), Order::<OverArchingCallOf<T>>::Null)
		};
	}: {
		if T::fungible_asset(1).is_some() {
			assert_ok!(execute_order::<T>(origin, holding, order));
		}
	} verify {
		if T::fungible_asset(1).is_some() {
			assert!(!T::FungibleTransactAsset::balance(&account::<T>(77)).is_zero())
		}
	}

	order_deposit_asset_fungibles_per_asset {
		// create one asset with our desired id.
		let asset_id = 9;
		let origin = MultiLocation::X1(account_id_junction::<T>(1));

		let (holding, order) = if let Some((asset, amount)) = T::fungibles_asset(1, asset_id) {
			let order = Order::<OverArchingCallOf<T>>::DepositAsset {
				assets: vec![ asset.clone() ],
				dest: MultiLocation::X1(account_id_junction::<T>(2)),
			};
			let mut holding: Assets = create_holding(HOLDING_FUNGIBLES, amount, HOLDING_NON_FUNGIBLES);
			holding.saturating_subsume(asset);
			assert!(T::FungibleTransactAsset::balance(&account::<T>(2)).is_zero());

			(holding, order)
		} else {
			(Default::default(), Order::<OverArchingCallOf<T>>::Null)
		};
	}: {
		if T::fungibles_asset(1, asset_id).is_some() {
			assert_ok!(execute_order::<T>(origin, holding, order));
		}
	} verify {
		if T::fungibles_asset(1, asset_id).is_some() {
			assert!(!T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero())
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

	xcm_withdraw_asset_fungible {
		for n in 0 .. 100;
		let amount = T::FungiblesTransactAsset::minimum_balance();
		for i in 0 .. n {
			let user = account::<T>(1337);
			assert_ok!(T::FungibleTransactAsset::mint_into(user, amount));
			assert_eq!(T::FungiblesTransactAsset::balance_of(user), amount);

			let multi_asset = T::get_id()
		}

		let xcm = Xcm::WithdrawAsset {
			assets: vec![multi_asset], // this is not full complexity
			effects: vec![],
		};

	}: {
		let holding = T::XcmExecutor::execute_and_return_holding(xcm)?;
	} verify {
		assert!(T::FungiblesTransactAsset::balance_of(user).is_zero());
		assert!(holding.contains(multi_asset, amount));
	}

	Xcm::WithdrawAsset { assest } => {
		assets.iter().map(|a| (a, Lookup::lookup(a))).for_each(|(asset, type)| {
			match type {
				AssetType::Fungible =>
			}
		})
	}

	xcm_withdraw_asset_fungibles {
		let n in 0 .. 1000;

		let user

		for n
	}: {} verify {}

	xcm_reserve_asset_deposit {}: {} verify {}
	xcm_teleport_asset {}: {} verify {}
	xcm_query_response {}: {} verify {}
	xcm_transfer_asset_fungible {
		let origin: MultiLocation = (account_id_junction::<T>(1)).into();
		let dest = account_id_junction::<T>(2).into();

		let xcm = if let Some((asset, amount)) = T::fungible_asset(1) {
			<AssetTransactorOf<T>>::deposit_asset(&asset, &origin).unwrap();
			let assets = vec![ asset ];
			assert!(T::FungibleTransactAsset::balance(&account::<T>(2)).is_zero());
			Xcm::TransferAsset { assets, dest }
		} else {
			Xcm::TransferAsset { assets: Default::default(), dest }
		};
	}: {
		if T::fungible_asset(1).is_some() {
			assert_ok!(execute_xcm::<T>(origin, xcm).ensure_complete());
		}
	} verify {
		if T::fungible_asset(1).is_some() {
			assert!(!T::FungibleTransactAsset::balance(&account::<T>(2)).is_zero());
		}
	}
	xcm_transfer_asset_fungibles_per_asset {
		let origin: MultiLocation = (account_id_junction::<T>(1)).into();
		let dest = (account_id_junction::<T>(2)).into();
		let asset_id = 9;

		let xcm = if let Some((asset, amount)) = T::fungibles_asset(1, asset_id) {
			// Note that we deposit a new asset with twice the amount into the sender to prevent it
			// being dying.
			<AssetTransactorOf<T>>::deposit_asset(
				&T::fungibles_asset(2, asset_id)
					.expect("call to fungibles_asset has already returned `Some`, this must work")
					.0,
				&origin
			).unwrap();

			assert!(T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero());
			assert!(!T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(1)).is_zero());

			Xcm::TransferAsset { assets: vec![asset], dest }
		} else {
			Xcm::TransferAsset { assets: Default::default(), dest }
		};
	}: {
		if T::fungibles_asset(0, 0).is_some() {
			assert_ok!(execute_xcm::<T>(origin, xcm).ensure_complete());
		}
	} verify {
		if T::fungibles_asset(0, 0).is_some() {
			assert!(!T::FungiblesTransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero());
		}
	}

	xcm_transfer_reserved_asset_fungible {}: {} verify {}
	xcm_transfer_reserved_asset_fungibles {}: {} verify {}

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
			test_bench_by_name::<crate::mock_fungible::Test>(b"order_deposit_asset_fungible")
				.unwrap();
		})
	}

	#[test]
	fn order_deposit_asset_fungibles_per_asset() {
		crate::mock_fungibles::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungibles::Test>(
				b"order_deposit_asset_fungibles_per_asset",
			)
			.unwrap();
		})
	}

	#[test]
	fn xcm_transfer_asset_fungible() {
		crate::mock_fungible::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungible::Test>(b"xcm_transfer_asset_fungible")
				.unwrap();
		})
	}
	#[test]
	fn xcm_transfer_asset_fungibles_per_asset() {
		crate::mock_fungibles::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungibles::Test>(
				b"xcm_transfer_asset_fungibles_per_asset",
			)
			.unwrap();
		})
	}

	#[test]
	fn xcm_withdraw_asset_fungible() {
		crate::mock_fungible::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungible::Test>(b"xcm_withdraw_asset_fungible")
				.unwrap();
		})
	}

	#[test]
	fn xcm_withdraw_asset_fungibles() {
		crate::mock_fungibles::new_test_ext().execute_with(|| {
			test_bench_by_name::<crate::mock_fungibles::Test>(b"xcm_withdraw_asset_fungibles")
				.unwrap();
		})
	}
}
