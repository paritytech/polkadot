// Copyright (C) Parity Technologies (UK) Ltd.
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
use crate::{account_and_location, new_executor, AssetTransactorOf, XcmCallOf};
use frame_benchmarking::{benchmarks_instance_pallet, BenchmarkError, BenchmarkResult};
use frame_support::{
	pallet_prelude::Get,
	traits::fungible::{Inspect, Mutate},
	weights::Weight,
};
use sp_runtime::traits::{Bounded, Zero};
use sp_std::{prelude::*, vec};
use xcm::latest::prelude::*;
use xcm_executor::traits::{ConvertLocation, TransactAsset};

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
		let worst_case_holding = T::worst_case_holding(0);
		let asset = T::get_multi_asset();

		<AssetTransactorOf<T>>::deposit_asset(
			&asset,
			&sender_location,
			&XcmContext {
				origin: Some(sender_location.clone()),
				message_id: [0; 32],
				topic: None,
			},
		).unwrap();
		// check the assets of origin.
		assert!(!T::TransactAsset::balance(&sender_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		executor.set_holding(worst_case_holding.into());
		let instruction = Instruction::<XcmCallOf<T>>::WithdrawAsset(vec![asset.clone()].into());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// check one of the assets of origin.
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(executor.holding().ensure_contains(&vec![asset].into()).is_ok());
	}

	transfer_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let asset = T::get_multi_asset();
		let assets: MultiAssets = vec![ asset.clone() ].into();
		// this xcm doesn't use holding

		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert_location(&dest_location).unwrap();

		<AssetTransactorOf<T>>::deposit_asset(
			&asset,
			&sender_location,
			&XcmContext {
				origin: Some(sender_location.clone()),
				message_id: [0; 32],
				topic: None,
			},
		).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		let instruction = Instruction::TransferAsset { assets, beneficiary: dest_location };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
	}

	transfer_reserve_asset {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert_location(&dest_location).unwrap();

		let asset = T::get_multi_asset();
		<AssetTransactorOf<T>>::deposit_asset(
			&asset,
			&sender_location,
			&XcmContext {
				origin: Some(sender_location.clone()),
				message_id: [0; 32],
				topic: None,
			},
		).unwrap();
		let assets: MultiAssets = vec![ asset ].into();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(sender_location);
		let instruction = Instruction::TransferReserveAsset {
			assets,
			dest: dest_location,
			xcm: Xcm::new()
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert!(T::TransactAsset::balance(&sender_account).is_zero());
		assert!(!T::TransactAsset::balance(&dest_account).is_zero());
		// TODO: Check sender queue is not empty. #4426
	}

	reserve_asset_deposited {
		let (trusted_reserve, transferable_reserve_asset) = T::TrustedReserve::get()
			.ok_or(BenchmarkError::Override(
				BenchmarkResult::from_weight(Weight::MAX)
			))?;

		let assets: MultiAssets = vec![ transferable_reserve_asset ].into();

		let mut executor = new_executor::<T>(trusted_reserve);
		let instruction = Instruction::ReserveAssetDeposited(assets.clone());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert!(executor.holding().ensure_contains(&assets).is_ok());
	}

	initiate_reserve_withdraw {
		let holding = T::worst_case_holding(1);
		let assets_filter = MultiAssetFilter::Definite(holding.clone());
		let reserve = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());
		let instruction = Instruction::InitiateReserveWithdraw { assets: assets_filter, reserve, xcm: Xcm(vec![]) };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// The execute completing successfully is as good as we can check.
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	receive_teleported_asset {
		// If there is no trusted teleporter, then we skip this benchmark.
		let (trusted_teleporter, teleportable_asset) = T::TrustedTeleporter::get()
			.ok_or(BenchmarkError::Skip)?;

		if let Some((checked_account, _)) = T::CheckedAccount::get() {
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
		executor.bench_process(xcm).map_err(|_| {
			BenchmarkError::Override(
				BenchmarkResult::from_weight(Weight::MAX)
			)
		})?;
	} verify {
		assert!(executor.holding().ensure_contains(&assets).is_ok());
	}

	deposit_asset {
		let asset = T::get_multi_asset();
		let mut holding = T::worst_case_holding(1);

		// Add our asset to the holding.
		holding.push(asset.clone());

		// our dest must have no balance initially.
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert_location(&dest_location).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());
		let instruction = Instruction::<XcmCallOf<T>>::DepositAsset {
			assets: asset.into(),
			beneficiary: dest_location,
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&dest_account).is_zero())
	}

	deposit_reserve_asset {
		let asset = T::get_multi_asset();
		let mut holding = T::worst_case_holding(1);

		// Add our asset to the holding.
		holding.push(asset.clone());

		// our dest must have no balance initially.
		let dest_location = T::valid_destination()?;
		let dest_account = T::AccountIdConverter::convert_location(&dest_location).unwrap();
		assert!(T::TransactAsset::balance(&dest_account).is_zero());

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());
		let instruction = Instruction::<XcmCallOf<T>>::DepositReserveAsset {
			assets: asset.into(),
			dest: dest_location,
			xcm: Xcm::new(),
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// dest should have received some asset.
		assert!(!T::TransactAsset::balance(&dest_account).is_zero())
	}

	initiate_teleport {
		let asset = T::get_multi_asset();
		let mut holding = T::worst_case_holding(0);

		// Add our asset to the holding.
		holding.push(asset.clone());

		// Checked account starts at zero
		assert!(T::CheckedAccount::get().map_or(true, |(c, _)| T::TransactAsset::balance(&c).is_zero()));

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());
		let instruction = Instruction::<XcmCallOf<T>>::InitiateTeleport {
			assets: asset.into(),
			dest: T::valid_destination()?,
			xcm: Xcm::new(),
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		if let Some((checked_account, _)) = T::CheckedAccount::get() {
			// teleport checked account should have received some asset.
			assert!(!T::TransactAsset::balance(&checked_account).is_zero());
		}
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::fungible::mock::new_test_ext(),
		crate::fungible::mock::Test
	);
}
