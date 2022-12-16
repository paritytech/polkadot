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

#[allow(deprecated)]
use crate::disputes::{pallet::SpamSlots, Config, Pallet};
use frame_support::{
	pallet_prelude::*,
	traits::{OnRuntimeUpgrade, StorageVersion},
	weights::Weight,
};
/// The current storage version.
const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
	fn on_runtime_upgrade() -> Weight {
		let mut weight: Weight = Weight::zero();

		if StorageVersion::get::<Pallet<T>>() < STORAGE_VERSION {
			weight += migrate_to_v1::<T>();
			STORAGE_VERSION.put::<Pallet<T>>();
		}
		weight
	}
}

/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
pub fn migrate_to_v1<T: Config>() -> Weight {
	let weight: Weight = Weight::zero();

	// SpamSlots should not contain too many keys so removing everything at once should be safe
	loop {
		#[allow(deprecated)]
		let res = SpamSlots::<T>::clear(0, None);
		// `loops` is the number of iterations => used to calculate read weights
		// `backend` is the number of keys removed from the backend => used to calculate write weights
		weight
			.saturating_add(T::DbWeight::get().reads_writes(res.loops as u64, res.backend as u64));

		if res.maybe_cursor.is_none() {
			break
		}
	}

	weight
}
