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

use super::*;
use frame_support::{
	traits::{Get, StorageVersion, GetStorageVersion},
	weights::Weight,
};
// simply add into the storage the MaxPermanentSlots and MaxTemporarySlots set on the old migration
pub fn migrate_to_v2<T: Config>() -> Weight {
    let onchain_version =  Pallet::<T>::on_chain_storage_version();
    if onchain_version < 2 {
        const MAX_PERMANENT_SLOTS: u32 = 100;
        const MAX_TEMPORARY_SLOTS: u32 = 100;

        <MaxPermanentSlots<T>>::put(MAX_PERMANENT_SLOTS);
        <MaxTemporarySlots<T>>::put(MAX_TEMPORARY_SLOTS);
        // Update storage version.
		StorageVersion::new(2).put::<Pallet::<T>>();
        // Return the weight consumed by the migration.
		T::DbWeight::get().reads_writes(1, 3)
    }
    else{
        Weight::zero()
    }
}