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

use super::{Config, MaxPermanentSlots, MaxTemporarySlots, Pallet};
use frame_support::{
	dispatch::GetStorageVersion,
	traits::{Get, OnRuntimeUpgrade, StorageVersion},
	weights::Weight,
};

pub mod v1 {

	use super::*;
	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
			let onchain_version = Pallet::<T>::on_chain_storage_version();
			ensure!(onchain_version < 1, "assigned_slots::MigrateToV1 migration can be deleted");
			Ok(Default::default())
		}

		fn on_runtime_upgrade() -> frame_support::weights::Weight {
			let onchain_version = Pallet::<T>::on_chain_storage_version();
			if onchain_version < 1 {
				const MAX_PERMANENT_SLOTS: u32 = 100;
				const MAX_TEMPORARY_SLOTS: u32 = 100;

				<MaxPermanentSlots<T>>::put(MAX_PERMANENT_SLOTS);
				<MaxTemporarySlots<T>>::put(MAX_TEMPORARY_SLOTS);
				// Update storage version.
				StorageVersion::new(1).put::<Pallet<T>>();
				// Return the weight consumed by the migration.
				T::DbWeight::get().reads_writes(1, 3)
			} else {
				Weight::zero()
			}
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
			let onchain_version = Pallet::<T>::on_chain_storage_version();
			ensure!(onchain_version == 1, "assigned_slots::MigrateToV1 needs to be run");
			Ok(())
		}
	}
}
