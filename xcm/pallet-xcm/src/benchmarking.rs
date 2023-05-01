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
use bounded_collections::{ConstU32, WeakBoundedVec};
use frame_benchmarking::{benchmarks, BenchmarkError, BenchmarkResult};
use frame_support::weights::Weight;
use frame_system::RawOrigin;
use sp_std::prelude::*;
use xcm::{latest::prelude::*, v2};

type RuntimeOrigin<T> = <T as frame_system::Config>::RuntimeOrigin;

benchmarks! {
	send {
		let send_origin =
			T::SendXcmOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		if T::SendXcmOrigin::try_origin(send_origin.clone()).is_err() {
			return Err(BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))
		}
		let msg = Xcm(vec![ClearOrigin]);
		let versioned_dest: VersionedMultiLocation = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?
		.into();
		let versioned_msg = VersionedXcm::from(msg);
	}: _<RuntimeOrigin<T>>(send_origin, Box::new(versioned_dest), Box::new(versioned_msg))

	teleport_assets {
		let asset: MultiAsset = (Here, 10).into();
		let send_origin =
			T::ExecuteXcmOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		let origin_location = T::ExecuteXcmOrigin::try_origin(send_origin.clone())
			.map_err(|_| BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))?;
		if !T::XcmTeleportFilter::contains(&(origin_location, vec![asset.clone()])) {
			return Err(BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))
		}

		let recipient = [0u8; 32];
		let versioned_dest: VersionedMultiLocation = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?
		.into();
		let versioned_beneficiary: VersionedMultiLocation =
			AccountId32 { network: None, id: recipient.into() }.into();
		let versioned_assets: VersionedMultiAssets = asset.into();
	}: _<RuntimeOrigin<T>>(send_origin, Box::new(versioned_dest), Box::new(versioned_beneficiary), Box::new(versioned_assets), 0)

	reserve_transfer_assets {
		let asset: MultiAsset = (Here, 10).into();
		let send_origin =
			T::ExecuteXcmOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		let origin_location = T::ExecuteXcmOrigin::try_origin(send_origin.clone())
			.map_err(|_| BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))?;
		if !T::XcmReserveTransferFilter::contains(&(origin_location, vec![asset.clone()])) {
			return Err(BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))
		}

		let recipient = [0u8; 32];
		let versioned_dest: VersionedMultiLocation = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?
		.into();
		let versioned_beneficiary: VersionedMultiLocation =
			AccountId32 { network: None, id: recipient.into() }.into();
		let versioned_assets: VersionedMultiAssets = asset.into();
	}: _<RuntimeOrigin<T>>(send_origin, Box::new(versioned_dest), Box::new(versioned_beneficiary), Box::new(versioned_assets), 0)

	execute {
		let execute_origin =
			T::ExecuteXcmOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		let origin_location = T::ExecuteXcmOrigin::try_origin(execute_origin.clone())
			.map_err(|_| BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))?;
		let msg = Xcm(vec![ClearOrigin]);
		if !T::XcmExecuteFilter::contains(&(origin_location, msg.clone())) {
			return Err(BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))
		}
		let versioned_msg = VersionedXcm::from(msg);
	}: _<RuntimeOrigin<T>>(execute_origin, Box::new(versioned_msg), Weight::zero())

	force_xcm_version {
		let loc = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?;
		let xcm_version = 2;
	}: _(RawOrigin::Root, Box::new(loc), xcm_version)

	force_default_xcm_version {}: _(RawOrigin::Root, Some(2))

	force_subscribe_version_notify {
		let versioned_loc: VersionedMultiLocation = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?
		.into();
	}: _(RawOrigin::Root, Box::new(versioned_loc))

	force_unsubscribe_version_notify {
		let loc = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)),
		)?;
		let versioned_loc: VersionedMultiLocation = loc.into();
		let _ = Pallet::<T>::request_version_notify(loc);
	}: _(RawOrigin::Root, Box::new(versioned_loc))

	force_suspension {}: _(RawOrigin::Root, true)

	migrate_supported_version {
		let old_version = XCM_VERSION - 1;
		let loc = VersionedMultiLocation::from(MultiLocation::from(Parent));
		SupportedVersion::<T>::insert(old_version, loc, old_version);
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::MigrateSupportedVersion, Weight::zero());
	}

	migrate_version_notifiers {
		let old_version = XCM_VERSION - 1;
		let loc = VersionedMultiLocation::from(MultiLocation::from(Parent));
		VersionNotifiers::<T>::insert(old_version, loc, 0);
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::MigrateVersionNotifiers, Weight::zero());
	}

	already_notified_target {
		let loc = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(T::DbWeight::get().reads(1))),
		)?;
		let loc = VersionedMultiLocation::from(loc);
		let current_version = T::AdvertisedXcmVersion::get();
		VersionNotifyTargets::<T>::insert(current_version, loc, (0, Weight::zero(), current_version));
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::NotifyCurrentTargets(None), Weight::zero());
	}

	notify_current_targets {
		let loc = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(T::DbWeight::get().reads_writes(1, 3))),
		)?;
		let loc = VersionedMultiLocation::from(loc);
		let current_version = T::AdvertisedXcmVersion::get();
		let old_version = current_version - 1;
		VersionNotifyTargets::<T>::insert(current_version, loc, (0, Weight::zero(), old_version));
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::NotifyCurrentTargets(None), Weight::zero());
	}

	notify_target_migration_fail {
		let bad_loc: v2::MultiLocation = v2::Junction::Plurality {
			id: v2::BodyId::Named(WeakBoundedVec::<u8, ConstU32<32>>::try_from(vec![0; 32])
				.expect("vec has a length of 32 bits; qed")),
			part: v2::BodyPart::Voice,
		}
		.into();
		let bad_loc = VersionedMultiLocation::from(bad_loc);
		let current_version = T::AdvertisedXcmVersion::get();
		VersionNotifyTargets::<T>::insert(current_version, bad_loc, (0, Weight::zero(), current_version));
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::MigrateAndNotifyOldTargets, Weight::zero());
	}

	migrate_version_notify_targets {
		let current_version = T::AdvertisedXcmVersion::get();
		let old_version = current_version - 1;
		let loc = VersionedMultiLocation::from(MultiLocation::from(Parent));
		VersionNotifyTargets::<T>::insert(old_version, loc, (0, Weight::zero(), current_version));
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::MigrateAndNotifyOldTargets, Weight::zero());
	}

	migrate_and_notify_old_targets {
		let loc = T::ReachableDest::get().ok_or(
			BenchmarkError::Override(BenchmarkResult::from_weight(T::DbWeight::get().reads_writes(1, 3))),
		)?;
		let loc = VersionedMultiLocation::from(loc);
		let old_version = T::AdvertisedXcmVersion::get() - 1;
		VersionNotifyTargets::<T>::insert(old_version, loc, (0, Weight::zero(), old_version));
	}: {
		Pallet::<T>::check_xcm_version_change(VersionMigrationStage::MigrateAndNotifyOldTargets, Weight::zero());
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext_with_balances(Vec::new()),
		crate::mock::Test
	);
}
