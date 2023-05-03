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

use frame_support::{
	storage::storage_prefix,
	traits::{Get, OnRuntimeUpgrade},
	weights::Weight,
};
use pallet_session::Config;
use sp_io::{storage::clear_prefix, KillStorageResult};
#[cfg(feature = "try-runtime")]
use sp_std::vec::Vec;

/// This migration clears everything under `Session::HistoricalSessions`
/// and `Session::StoredRange` that were not cleared when
/// the pallet was moved to its new prefix (`Historical`)  
pub struct ClearOldSessionStorage<T>(sp_std::marker::PhantomData<T>);
impl<T: Config> OnRuntimeUpgrade for ClearOldSessionStorage<T> {
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
