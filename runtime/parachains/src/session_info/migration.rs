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

//! A module that is responsible for migration of storage.

use frame_support::traits::StorageVersion;

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

pub mod v2 {
	use super::STORAGE_VERSION;
	use crate::{
		session_info,
		session_info::{Pallet, Store},
	};
	use frame_support::{
		pallet_prelude::Weight,
		traits::{OnRuntimeUpgrade, StorageVersion},
	};
	use frame_system::Config;
	use sp_core::Get;

	#[cfg(feature = "try-runtime")]
	use primitives::vstaging::ExecutorParams;

	#[cfg(feature = "try-runtime")]
	use crate::session_info::Vec;

	#[cfg(feature = "try-runtime")]
	use crate::shared;

	const LOG_TARGET: &'static str = "runtime::session_info::migration::v2";

	pub struct MigrateToV2<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config + session_info::pallet::Config> OnRuntimeUpgrade for MigrateToV2<T> {
		fn on_runtime_upgrade() -> Weight {
			let db_weight = T::DbWeight::get();
			let mut weight = db_weight.reads(1);
			if StorageVersion::get::<Pallet<T>>() == 1 {
				log::info!(target: LOG_TARGET, "Upgrading storage v1 -> v2");
				let mut vs = 0;

				<Pallet<T> as Store>::Sessions::translate_values(
					|old: primitives::v2::SessionInfo| {
						vs += 1;
						let new = primitives::v3::SessionInfo::from(old);
						Some(new)
					},
				);
				weight += db_weight.reads_writes(vs, vs);

				STORAGE_VERSION.put::<Pallet<T>>();
				weight += db_weight.writes(1);
			} else {
				log::warn!(target: LOG_TARGET, "Can only upgrade from version 1");
			}
			weight
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
			log::info!(target: LOG_TARGET, "Performing pre-upgrade checks");
			assert_eq!(StorageVersion::get::<Pallet<T>>(), 1);
			Ok(Default::default())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_: Vec<u8>) -> Result<(), &'static str> {
			log::info!(target: LOG_TARGET, "Performing post-upgrade checks");
			assert_eq!(StorageVersion::get::<Pallet<T>>(), 2);
			let session_index = <shared::Pallet<T>>::session_index();
			let session_info = Pallet::<T>::session_info(session_index);
			assert_eq!(session_info.unwrap().executor_params, ExecutorParams::default());
			Ok(())
		}
	}
}
