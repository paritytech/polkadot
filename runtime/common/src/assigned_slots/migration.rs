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

use super::{Config, MaxPermanentSlots, MaxTemporarySlots, Pallet, LOG_TARGET};
use frame_support::{
	dispatch::GetStorageVersion,
	traits::{Get, OnRuntimeUpgrade},
};

#[cfg(feature = "try-runtime")]
use frame_support::ensure;
#[cfg(feature = "try-runtime")]
use sp_std::vec::Vec;

pub mod v1 {

	use super::*;
	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
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
				// Return the weight consumed by the migration.
				T::DbWeight::get().reads_writes(1, 3)
			} else {
				log::info!(target: LOG_TARGET, "MigrateToV1 should be removed");
				T::DbWeight::get().reads(1)
			}
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			let onchain_version = Pallet::<T>::on_chain_storage_version();
			ensure!(onchain_version == 1, "assigned_slots::MigrateToV1 needs to be run");
			assert_eq!(<MaxPermanentSlots<T>>::get(), 100);
			assert_eq!(<MaxTemporarySlots<T>>::get(), 100);
			Ok(())
		}
	}

	/// [`VersionUncheckedMigrateToV1`] wrapped in a
	/// [`frame_support::migrations::VersionedRuntimeUpgrade`], ensuring the migration is only
	/// performed when on-chain version is 0.
	#[cfg(feature = "experimental")]
	pub type VersionCheckedMigrateToV1<T> = frame_support::migrations::VersionedRuntimeUpgrade<
		0,
		1,
		MigrateToV1<T>,
		Pallet<T>,
		<T as frame_system::Config>::DbWeight,
	>;
}
