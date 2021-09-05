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
	account, account_id_junction, execute_xcm, worst_case_holding, AssetTransactorOf, XcmCallOf,
};
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_support::{
	assert_ok,
	traits::fungibles::{Inspect, Mutate},
};
use sp_runtime::traits::Zero;
use sp_std::{convert::TryInto, prelude::*, vec};
use xcm::latest::prelude::*;
use xcm_executor::{traits::TransactAsset, Assets};

// TODO: def. needs to be become a config, might also want to use bounded vec.
const MAX_ASSETS: u32 = 25;

benchmarks! {
	where_clause {
		where <T::TransactAsset as Inspect<T::AccountId>>::AssetId: From<u32>,
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

	send_xcm {}: {}

	// orders.
	noop {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = Assets::default();
		let xcm = Xcm::<XcmCallOf<T>>::new();
	}: {
		execute_xcm::<T>(origin, holding, xcm)?;
	}

	deposit_asset_per_asset {
		// create one asset with our desired id.
		let asset_id = 9;
		let origin: MultiLocation = account_id_junction::<T>(1).into();

		let asset =  T::get_multi_asset(asset_id);
		let instruction = Instruction::<XcmCallOf<T>>::DepositAsset {
			assets: asset.clone().into(),
			max_assets: 1,
			beneficiary: account_id_junction::<T>(2).into(),
		};
		let xcm = Xcm(vec![instruction]);

		let amount: u128 = T::TransactAsset::minimum_balance(asset_id.into()).try_into().unwrap();
		let mut holding: Assets = worst_case_holding();
		holding.subsume(asset);
		assert!(T::TransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero());
	}: {
		execute_xcm::<T>(origin, holding, xcm)?;
	} verify {
		assert!(!T::TransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero())
	}

	deposit_reserve_asset {}: {} verify {}
	exchange_asset {}: {} verify {}
	initiate_reserve_withdraw {}: {} verify {}
	initiate_teleport {}: {} verify {}
	query_holding {}: {} verify {}
	buy_execution {}: {} verify {}

	withdraw_asset_per_asset {
		// number of fungible assets.
		let a in 1..MAX_ASSETS+1;

		let origin: MultiLocation = (account_id_junction::<T>(1)).into();
		let assets: MultiAssets = (1..=a).map(|i| {
			let asset = T::get_multi_asset(i);
			// give all of these assets to the origin.
			<AssetTransactorOf<T>>::deposit_asset(&asset, &origin).unwrap();
			asset
		})
		.collect::<Vec<_>>().into();
		// check just one of the asset ids, namely 1.
		assert!(!T::TransactAsset::balance(1u32.into(), &account::<T>(1)).is_zero());
		let instruction = Instruction::<XcmCallOf<T>>::WithdrawAsset(assets);
		let xcm = Xcm(vec![instruction]);
	}: {
		execute_xcm::<T>(origin, Default::default(), xcm)?;
	} verify {
		// check one of the assets of origin. All assets must have been withdrawn.
		assert!(T::TransactAsset::balance(1u32.into(), &account::<T>(1)).is_zero());
	}
	reserve_asset_deposit {}: {} verify {}
	receive_teleported_asset {}: {} verify {}
	transfer_asset_per_asset {
		let a in 1..MAX_ASSETS+1;

		let origin: MultiLocation = (account_id_junction::<T>(1)).into();
		let beneficiary = (account_id_junction::<T>(2)).into();

		let assets: MultiAssets = (1..a).map(|asset_id| {
			let asset = T::get_multi_asset(asset_id);
			// Note that we deposit a new asset with twice the amount into the sender to prevent it
			// being dying.
			let amount = T::TransactAsset::minimum_balance(asset_id.into()) * 2u32.into();
			assert_ok!(T::TransactAsset::mint_into(asset_id.into(), &account::<T>(1), amount));

			// verify accounts funded
			assert!(T::TransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero());
			assert!(!T::TransactAsset::balance(asset_id.into(), &account::<T>(1)).is_zero());

			// return asset
			asset
		})
		.collect::<Vec<_>>().into();

		let instruction = Instruction::TransferAsset { assets, beneficiary };
		let xcm = Xcm(vec![instruction]);
	}: {
		execute_xcm::<T>(origin, Default::default(), xcm)?;
	} verify {
		for asset_id in 1..a {
			assert!(!T::TransactAsset::balance(asset_id.into(), &account::<T>(2)).is_zero());
		}
	}
	transfer_reserve_asset {}: {} verify {}
}

impl_benchmark_test_suite!(
	Pallet,
	crate::fungibles::mock::new_test_ext(),
	crate::fungibles::mock::Test
);
