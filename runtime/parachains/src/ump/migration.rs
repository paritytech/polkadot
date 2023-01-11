// Copyright 2022 Parity Technologies (UK) Ltd.
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

use crate::ump::{Config, Overweight, Pallet};
use frame_support::{
	pallet_prelude::*,
	traits::{OnRuntimeUpgrade, StorageVersion},
	weights::Weight,
};

pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v1 {
	use super::*;

	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		fn on_runtime_upgrade() -> Weight {
			if StorageVersion::get::<Pallet<T>>() == 0 {
				let mut weight = T::DbWeight::get().reads(1);

				let overweight_messages = Overweight::<T>::initialize_counter() as u64;
				log::info!("Initialized Overweight to {}", overweight_messages);

				weight.saturating_accrue(T::DbWeight::get().reads_writes(overweight_messages, 1));

				StorageVersion::new(1).put::<Pallet<T>>();

				weight.saturating_add(T::DbWeight::get().writes(1))
			} else {
				log::warn!("skipping v1, should be removed");
				T::DbWeight::get().reads(1)
			}
		}
	}
}
