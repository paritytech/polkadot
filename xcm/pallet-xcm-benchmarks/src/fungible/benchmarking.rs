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
#[cfg(any(test, feature = "runtime-benchmarks"))]
use crate::mock::sent_xcm;
use crate::{account_and_location, new_executor, AssetTransactorOf, XcmCallOf};
use frame_benchmarking::{benchmarks_instance_pallet, BenchmarkError, BenchmarkResult};
use frame_support::{
	pallet_prelude::Get,
	traits::fungible::{Inspect, Mutate},
};
use sp_runtime::traits::{Bounded, Zero};
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

	withdraw_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let worst_case_holding = T::worst_case_holding();
		let asset = T::get_multi_asset();

		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();
		// check the assets of origin.
		assert!(!T::TransactAsset::balance(&sender_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		executor.holding = worst_case_holding.into();
		let instruction = Instruction::<XcmCallOf<T>>::WithdrawAsset(vec![asset.clone()].into());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		// check one of the assets of origin.
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(executor.holding.ensure_contains(&vec![asset].into()).is_ok());
	}

	transfer_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let asset = T::get_multi_asset();
		let assets: MultiAssets = vec![ asset.clone() ].into();
		// this xcm doesn't use holding

		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();

		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		let instruction = Instruction::TransferAsset { assets, beneficiary: dest_location };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
	}

	transfer_reserve_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();

		let asset = T::get_multi_asset();
		<AssetTransactorOf<T>>::deposit_asset(&asset, &sender_location).unwrap();
		let mut assets: MultiAssets = vec![ asset ].into();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		let instruction = Instruction::TransferReserveAsset {
			assets: assets.clone(),
			dest: dest_location.clone(),
			xcm: Xcm::new()
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		let _ = assets.reanchor(&dest_location, &Here.into());
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
		assert_eq!(sent_xcm(), vec![(
			dest_location,
			Xcm(vec![
				Instruction::ReserveAssetDeposited(assets),
				Instruction::ClearOrigin,
			]),
		)]);
	}

	receive_teleported_asset {
		// If there is no trusted teleporter, then we skip this benchmark.
		let (trusted_teleporter, teleportable_asset) = T::TrustedTeleporter::get()
			.ok_or(BenchmarkError::Skip)?;

		if let Some(checked_account) = T::CheckedAccount::get() {
			T::TransactAsset::mint_into(
				&checked_account,
				<
					T::TransactAsset
					as
					Inspect<T::AccountId>
				>::Balance::max_value() / 2u32.into(),
			)?;
		}

		let assets: MultiAssets = vec![ teleportable_asset ].into();

		let mut executor = new_executor::<T>(trusted_teleporter);
		let instruction = Instruction::ReceiveTeleportedAsset(assets.clone());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm).map_err(|_| {
			BenchmarkError::Override(
				BenchmarkResult::from_weight(T::BlockWeights::get().max_block)
			)
		})?;
	} verify {
		assert!(executor.holding.ensure_contains(&assets).is_ok());
	}

	deposit_asset {
		let asset = T::get_multi_asset();
		let mut holding = T::worst_case_holding();

		// Add our asset to the holding.
		holding.push(asset.clone());

		// our dest must have no balance initially.
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding.into();
		let instruction = Instruction::<XcmCallOf<T>>::DepositAsset {
			assets: asset.into(),
			max_assets: 1,
			beneficiary: dest_location,
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&dest_account).is_zero())
	}

	deposit_reserve_asset {
		let asset = T::get_multi_asset();
		let mut holding = T::worst_case_holding();

		// Add our asset to the holding.
		holding.push(asset.clone());

		// our dest must have no balance initially.
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert(dest_location.clone()).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut assets: MultiAssets = asset.into();

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding.into();
		let instruction = Instruction::<XcmCallOf<T>>::DepositReserveAsset {
			assets: assets.clone().into(),
			max_assets: 1,
			dest: dest_location.clone(),
			xcm: Xcm::new(),
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		let _ = assets.reanchor(&dest_location, &Here.into());
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
		assert_eq!(sent_xcm(), vec![(
			dest_location,
			Xcm(vec![
				Instruction::ReserveAssetDeposited(assets),
				Instruction::ClearOrigin,
			]),
		)]);
	}

	initiate_teleport {
		let mut asset = T::get_multi_asset();
		let dest = T::valid_destination()?;
		let mut holding = T::worst_case_holding();

		// Add our asset to the holding.
		holding.push(asset.clone());

		// Checked account starts at zero
		assert!(T::CheckedAccount::get().map_or(true, |c| T::TransactAsset::balance(&c).is_zero()));

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding.into();
		let instruction = Instruction::<XcmCallOf<T>>::InitiateTeleport {
			assets: asset.clone().into(),
			dest: dest.clone(),
			xcm: Xcm::new(),
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		let _ = asset.reanchor(&dest, &Here.into());
		if let Some(checked_account) = T::CheckedAccount::get() {
			// teleport checked account should have received some asset.
			assert!(!T::TransactAsset::balance(&checked_account).is_zero());
		}
		assert_eq!(sent_xcm(), vec![(
			dest,
			Xcm(vec![
				Instruction::ReceiveTeleportedAsset(asset.into()),
				Instruction::ClearOrigin,
			]),
		)]);
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::fungible::mock::new_test_ext(),
		crate::fungible::mock::Test
	);
}
