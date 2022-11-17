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
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v3 {
	use crate::{session_info, shared};
	use frame_support::{pallet_prelude::Weight, traits::OnRuntimeUpgrade};
	use frame_system::Config;
	use primitives::vstaging::ExecutorParams;
	use sp_core::Get;

	pub struct MigrateToV3<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config + session_info::pallet::Config> OnRuntimeUpgrade for MigrateToV3<T> {
		fn on_runtime_upgrade() -> Weight {
			// Bootstrap session executor params with the default ones if no parameters for the
			// current session are in storage. `ExecutorParams::default()` is supposed to generate
			// EXACTLY the same set of parameters the previous implementation used in a hard-coded
			// form. This supposed to only run once, when upgrading from pre-parametrized executor
			// code.
			let mut weight = T::DbWeight::get().reads(2);
			let session_index = <shared::Pallet<T>>::session_index();
			if <session_info::Pallet<T>>::session_executor_params(session_index).is_none() {
				session_info::pallet::SessionExecutorParams::<T>::insert(
					&session_index,
					ExecutorParams::default(),
				);
				weight += T::DbWeight::get().writes(1);
			}
			weight
		}
	}
}
