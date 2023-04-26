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

use pallet_session::Config;
use frame_support::{
    traits::{Get, OnRuntimeUpgrade},
    storage::storage_prefix,
    weights::Weight,
};
use sp_io::{storage::clear_prefix, KillStorageResult};

pub struct MigrateToV2<T>(sp_std::marker::PhantomData<T>);
impl<T: Config> OnRuntimeUpgrade for MigrateToV2<T> {
    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
        Ok(Vec::new())
    }

    fn on_runtime_upgrade() -> Weight {
        let prefix = storage_prefix(b"Session", b"StoredRange");
        let keys_removed_stored_range = match clear_prefix(&prefix, None) {
			KillStorageResult::AllRemoved(value) => value,
			KillStorageResult::SomeRemaining(value) => {
				log::error!(
					"`clear_prefix` failed to remove all keys. THIS SHOULD NEVER HAPPEN! 🚨",
				);
				value
			},
		} as u64;

        let prefix = storage_prefix(b"Session", b"HistoricalSessions");
        let keys_removed_historical_sessions = match clear_prefix(&prefix, None) {
			KillStorageResult::AllRemoved(value) => value,
			KillStorageResult::SomeRemaining(value) => {
				log::error!(
					"`clear_prefix` failed to remove all keys. THIS SHOULD NEVER HAPPEN! 🚨",
				);
				value
			},
		} as u64;

        let keys_removed = keys_removed_stored_range + keys_removed_historical_sessions;
		log::info!("Removed {} keys 🧹", keys_removed);

        T::DbWeight::get().reads_writes(keys_removed, keys_removed)
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
        Ok(())
    }
}

#[cfg(test)]
mod test {
	use super::*;
    use crate::mock::{new_test_ext, Test as T};
    use frame_support::{weights::RuntimeDbWeight, Twox64Concat};

    #[test]
	fn migration_to_v1_works() {
        #[frame_support::storage_alias]
        type HistoricalSessions = StorageMap<Session, Twox64Concat, u64, u64>;

        #[frame_support::storage_alias]
        type StoredRange = StorageValue<Session, u64>;

		let mut ext = new_test_ext();

		ext.execute_with(|| {
			HistoricalSessions::insert(1, 10);
            StoredRange::set(Some(5));

			assert!(HistoricalSessions::iter_keys().collect::<Vec<_>>().len() > 0);
            assert_eq!(StoredRange::get(), Some(5));
		});

		ext.commit_all().unwrap();

		ext.execute_with(|| {
            let weight: RuntimeDbWeight = <T as frame_system::Config>::DbWeight::get();

			assert_eq!(
				MigrateToV2::<T>::on_runtime_upgrade(),
				weight.reads_writes(2, 2),
			);

			assert!(HistoricalSessions::iter_keys().collect::<Vec<_>>().len() == 0);
            assert_eq!(StoredRange::get(), None);
		})
	}
}