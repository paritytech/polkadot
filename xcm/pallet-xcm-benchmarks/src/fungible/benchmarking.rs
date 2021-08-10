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
	account_and_location, account_id_junction, execute_order, execute_xcm, worst_case_holding,
	AssetTransactorOf, XcmCallOf,
};
use frame_benchmarking::{benchmarks_instance_pallet, impl_benchmark_test_suite};
use frame_support::{assert_ok, pallet_prelude::Get, traits::fungible::Inspect};
use sp_runtime::traits::Zero;
use sp_std::{convert::TryInto, prelude::*, vec};
use xcm::latest::prelude::*;
use xcm_executor::traits::{Convert, TransactAsset};

benchmarks_instance_pallet! {
	where_clause { where
		<
			<
				T::TransactAsset
				as
				Inspect<T::AccountId>
			>::Balance
			as
			TryInto<u128>
		>::Error: sp_std::fmt::Debug,
	}

	order_deposit_asset {
		let origin: MultiLocation = account_id_junction::<T>(1).into();

		let asset = T::get_multi_asset();
		let amount: u128 = T::TransactAsset::minimum_balance().try_into().unwrap();
		// generate the holding with a bunch of stuff..
		let mut holding = worst_case_holding();
		// .. and the specific asset that we want to take out.
		holding.subsume(asset.clone());
		// our dest must have no balance initially.
		let ben_location = T::ValidDestination::get();
		let ben_account = T::AccountIdConverter::convert(ben_location.clone()).unwrap();
		assert!(T::TransactAsset::balance(&ben_account).is_zero());

		let order = Order::<XcmCallOf<T>>::DepositAsset {
			assets: asset.into(),
			max_assets: 1,
			beneficiary: ben_location,
		};
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&ben_account).is_zero())
	}

	order_deposit_reserve_asset {
		let origin: MultiLocation = account_id_junction::<T>(1).into();

		let asset = T::get_multi_asset();
		let amount: u128 = T::TransactAsset::minimum_balance().try_into().unwrap();
		// generate the holding with a bunch of stuff..
		// TODO: maybe update this api
		let mut holding = worst_case_holding();
		// .. and the specific asset that we want to take out.
		holding.subsume(asset.clone());
		// our dest must have no balance initially.
		let dest_location = T::ValidDestination::get();
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let effects = Vec::new(); // No effects to isolate the order
		let order = Order::<XcmCallOf<T>>::DepositReserveAsset {
			assets: asset.into(),
			max_assets: 1,
			dest: dest_location,
			effects,
		};
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&dest_account).is_zero())
	}

	order_initiate_reserve_withdraw {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let asset = T::get_multi_asset();
		let amount: u128 = T::TransactAsset::minimum_balance().try_into().unwrap();
		// generate the holding with a bunch of stuff..
		let mut holding = worst_case_holding();
		// .. and the specific asset that we want to take out.
		holding.subsume(asset.clone());

		let effects = Vec::new(); // No effects to isolate the order
		let order = Order::<XcmCallOf<T>>::DepositReserveAsset {
			assets: asset.into(),
			max_assets: 1,
			dest: T::ValidDestination::get(),
			effects,
		};
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		// execute_order succeeding is enough to verify this completed.
		// TODO: ideally we query DMP queue
	}

	order_initiate_teleport {
		let (sender_account, origin) = account_and_location::<T>(1);
		let asset = T::get_multi_asset();
		let amount: u128 = T::TransactAsset::minimum_balance().try_into().unwrap();
		// generate the holding with a bunch of stuff..
		let mut holding = worst_case_holding();
		// .. and the specific asset that we want to take out.
		holding.subsume(asset.clone());

		let effects = Vec::new(); // No effects to isolate the order
		let order = Order::<XcmCallOf<T>>::InitiateTeleport {
			assets: asset.into(),
			dest: T::ValidDestination::get(),
			effects,
		};
		if let Some(checked_account) = T::CheckedAccount::get() {
			// Checked account starts at zero
			assert!(T::TransactAsset::balance(&checked_account).is_zero());
		}
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		if let Some(checked_account) = T::CheckedAccount::get() {
			// teleport checked account should have received some asset.
			assert!(!T::TransactAsset::balance(&checked_account).is_zero());
		}
	}

	xcm_withdraw_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let asset = T::get_multi_asset();
		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();

		// TODO that this is sum-optimal. We should ideally populate the holding account, but we
		// can't, nor does it make sense. Insertion of assets into holding gets worse over time (log
		// of n), and we can't really capture this yet. We could still make a wrapper for
		// execute_xcm and hack around this. It will be like
		//
		// `fn execute_xcm_with_holding(xcm, holding)`
		//
		// and it will execute the xcm, assuming that the holding was already set. This only makes
		// sense by our assumption of xcm's being weighed by each of their asset. If you have 3
		// assets, while benchmarking the third, you artificially set some values into the holding,
		// which reflect the state of the holding after executing the previous two xcms.

		// check the assets of origin.
		assert!(!T::TransactAsset::balance(&sender_account).is_zero());
		// TODO: probably we can and should just use the opaque xcm/order types.
		let xcm = Xcm::WithdrawAsset::<XcmCallOf<T>> { assets: vec![asset].into(), effects: vec![] };
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		// check one of the assets of origin.
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
	}
	xcm_receive_teleported_asset {

	}: {} verify {}
	xcm_transfer_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let ben_location = T::ValidDestination::get();
		let ben_account = T::AccountIdConverter::convert(ben_location.clone()).unwrap();

		let asset = T::get_multi_asset();
		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();
		let assets: MultiAssets = vec![ asset ].into();
		assert!(T::TransactAsset::balance(&ben_account).is_zero());
		let xcm = Xcm::TransferAsset { assets, beneficiary: ben_location };
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&ben_account).is_zero());
	}

	xcm_transfer_reserve_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let dest_location = T::ValidDestination::get();
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();

		let asset = T::get_multi_asset();
		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();
		let assets: MultiAssets = vec![ asset ].into();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());
		let effects = Vec::new(); // No effects to isolate the xcm
		let xcm = Xcm::TransferReserveAsset { assets, dest: dest_location, effects };
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
	}
}

// TODO: this macro cannot be called alone, fix it in substrate.
impl_benchmark_test_suite!(
	Pallet,
	crate::fungible::mock::new_test_ext(),
	//westend_runtime::Runtime,
	crate::fungible::mock::Test
);
