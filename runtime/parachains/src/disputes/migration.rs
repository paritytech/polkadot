// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

//! Storage migration(s) related to disputes pallet

use frame_support::traits::StorageVersion;

/// The current storage version.
const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v1 {
	use super::*;
	use crate::disputes::{Config, Pallet};
	use frame_support::{
		pallet_prelude::*, storage_alias, traits::OnRuntimeUpgrade, weights::Weight,
	};
	use primitives::v2::SessionIndex;
	use sp_std::prelude::*;

	#[storage_alias]
	type SpamSlots<T: Config> = StorageMap<Pallet<T>, Twox64Concat, SessionIndex, Vec<u32>>;

	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut weight: Weight = Weight::zero();

			if StorageVersion::get::<Pallet<T>>() < STORAGE_VERSION {
				log::info!(target: crate::disputes::LOG_TARGET, "Migrating disputes storage to v1");
				weight += migrate_to_v1::<T>();
				STORAGE_VERSION.put::<Pallet<T>>();
				weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 1));
			} else {
				log::info!(
					target: crate::disputes::LOG_TARGET,
					"Disputes storage up to date - no need for migration"
				);
			}

			weight
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
			log::trace!(
				target: crate::disputes::LOG_TARGET,
				"SpamSlots before migration: {}",
				SpamSlots::<T>::iter().count()
			);
			ensure!(
				StorageVersion::get::<Pallet<T>>() == 0,
				"Storage version should be less than `1` before the migration",
			);
			Ok(Vec::new())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
			log::trace!(target: crate::disputes::LOG_TARGET, "Running post_upgrade()");
			ensure!(
				StorageVersion::get::<Pallet<T>>() == STORAGE_VERSION,
				"Storage version should be `1` after the migration"
			);
			ensure!(
				SpamSlots::<T>::iter().count() == 0,
				"SpamSlots should be empty after the migration"
			);
			Ok(())
		}
	}

	/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
	pub fn migrate_to_v1<T: Config>() -> Weight {
		let mut weight: Weight = Weight::zero();

		// SpamSlots should not contain too many keys so removing everything at once should be safe
		let res = SpamSlots::<T>::clear(u32::MAX, None);
		// `loops` is the number of iterations => used to calculate read weights
		// `backend` is the number of keys removed from the backend => used to calculate write weights
		weight = weight
			.saturating_add(T::DbWeight::get().reads_writes(res.loops as u64, res.backend as u64));

		weight
	}
}
