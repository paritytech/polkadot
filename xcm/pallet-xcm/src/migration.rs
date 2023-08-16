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

use crate::{Config, Pallet, VersionNotifyTargets};
use frame_support::{
	pallet_prelude::*,
	traits::{OnRuntimeUpgrade, StorageVersion},
	weights::Weight,
};

const DEFAULT_PROOF_SIZE: u64 = 64 * 1024;

pub mod v1 {
	use super::*;
	use crate::{CurrentMigration, VersionMigrationStage};

	/// Named with the 'VersionUnchecked'-prefix because although this implements some version
	/// checking, the version checking is not complete as it will begin failing after the upgrade is
	/// enacted on-chain.
	///
	/// Use experimental [`VersionCheckedMigrateToV1`] instead.
	pub struct VersionUncheckedMigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for VersionUncheckedMigrateToV1<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut weight = T::DbWeight::get().reads(1);

			if StorageVersion::get::<Pallet<T>>() != 0 {
				log::warn!("skipping v1, should be removed");
				return weight
			}

			weight.saturating_accrue(T::DbWeight::get().writes(1));
			CurrentMigration::<T>::put(VersionMigrationStage::default());

			let translate = |pre: (u64, u64, u32)| -> Option<(u64, Weight, u32)> {
				weight.saturating_accrue(T::DbWeight::get().reads_writes(1, 1));
				let translated = (pre.0, Weight::from_parts(pre.1, DEFAULT_PROOF_SIZE), pre.2);
				log::info!("Migrated VersionNotifyTarget {:?} to {:?}", pre, translated);
				Some(translated)
			};

			VersionNotifyTargets::<T>::translate_values(translate);

			log::info!("v1 applied successfully");
			weight.saturating_accrue(T::DbWeight::get().writes(1));
			StorageVersion::new(1).put::<Pallet<T>>();
			weight
		}
	}

	/// Version checked migration to v1.
	///
	/// Wrapped in VersionedRuntimeUpgrade so the pre/post checks don't begin failing after the
	/// upgrade is enacted on-chain.
	#[cfg(feature = "experimental")]
	pub type VersionCheckedMigrateToV1<T> = frame_support::migrations::VersionedRuntimeUpgrade<
		0,
		1,
		VersionUncheckedMigrateToV1<T>,
		crate::pallet::Pallet<T>,
		<T as frame_system::Config>::DbWeight,
	>;
}
