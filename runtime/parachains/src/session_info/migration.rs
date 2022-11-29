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
	use crate::{session_info, session_info::Pallet, shared};
	use frame_support::{
		pallet_prelude::Weight,
		traits::{OnRuntimeUpgrade, StorageVersion},
	};
	use frame_system::Config;
	use primitives::vstaging::ExecutorParams;
	use sp_core::Get;

	#[cfg(feature = "try-runtime")]
	use crate::session_info::Vec;

	pub struct MigrateToV2<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config + session_info::pallet::Config> OnRuntimeUpgrade for MigrateToV2<T> {
		fn on_runtime_upgrade() -> Weight {
			// Bootstrap session executor params with the default ones if no parameters for the
			// current session are in storage. `ExecutorParams::default()` is supposed to generate
			// EXACTLY the same set of parameters the previous implementation used in a hard-coded
			// form. This supposed to only run once, when upgrading from pre-parametrized executor
			// code.
			let db_weight = T::DbWeight::get();
			let mut weight = db_weight.reads(1);
			if StorageVersion::get::<Pallet<T>>() == 1 {
				let session_index = <shared::Pallet<T>>::session_index();
				session_info::pallet::SessionExecutorParams::<T>::insert(
					&session_index,
					ExecutorParams::default(),
				);
				STORAGE_VERSION.put::<Pallet<T>>();
				weight += db_weight.reads(1) + db_weight.writes(2);
			}
			weight
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
			assert_eq!(StorageVersion::get::<Pallet<T>>(), 1);
			let session_index = <shared::Pallet<T>>::session_index();
			assert!(Pallet::<T>::session_executor_params(session_index).is_none());
			Ok(Default::default())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_: Vec<u8>) -> Result<(), &'static str> {
			assert_eq!(StorageVersion::get::<Pallet<T>>(), 2);
			let session_index = <shared::Pallet<T>>::session_index();
			let executor_params = Pallet::<T>::session_executor_params(session_index);
			assert_eq!(executor_params, Some(ExecutorParams::default()));
			Ok(())
		}
	}
}
