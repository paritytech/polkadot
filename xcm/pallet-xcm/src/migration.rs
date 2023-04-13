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

pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

const DEFAULT_PROOF_SIZE: u64 = 64 * 1024;

pub mod v1 {
	use super::*;

	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<sp_std::vec::Vec<u8>, &'static str> {
			ensure!(StorageVersion::get::<Pallet<T>>() == 0, "must upgrade linearly");

			Ok(sp_std::vec::Vec::new())
		}

		fn on_runtime_upgrade() -> Weight {
			if StorageVersion::get::<Pallet<T>>() == 0 {
				let mut weight = T::DbWeight::get().reads(1);

				let translate = |pre: (u64, u64, u32)| -> Option<(u64, Weight, u32)> {
					weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 1));
					let translated = (pre.0, Weight::from_parts(pre.1, DEFAULT_PROOF_SIZE), pre.2);
					log::info!("Migrated VersionNotifyTarget {:?} to {:?}", pre, translated);
					Some(translated)
				};

				VersionNotifyTargets::<T>::translate_values(translate);

				log::info!("v1 applied successfully");
				STORAGE_VERSION.put::<Pallet<T>>();

				weight.saturating_add(T::DbWeight::get().writes(1))
			} else {
				log::warn!("skipping v1, should be removed");
				T::DbWeight::get().reads(1)
			}
		}
	}
}
