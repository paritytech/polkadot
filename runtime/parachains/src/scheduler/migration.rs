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

//! A module that is responsible for migration of storage.

use crate::scheduler::{self, AssignmentKind, Config, Pallet, Store};
use frame_support::{pallet_prelude::*, traits::StorageVersion, weights::Weight};
use parity_scale_codec::{Decode, Encode};

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

/// Call this during the next runtime upgrade for this module.
pub fn on_runtime_upgrade<T: Config>() -> Weight {
	let mut weight: Weight = 0;

	if StorageVersion::get::<Pallet<T>>() == 0 {
		weight = weight
			.saturating_add(v1::migrate::<T>())
			.saturating_add(T::DbWeight::get().writes(1));
		StorageVersion::new(1).put::<Pallet<T>>();
	}

	weight
}

mod v0 {
	use super::*;
	use primitives::v1::{CoreIndex, GroupIndex, Id as ParaId};

	#[derive(Encode, Decode)]
	pub struct CoreAssignment {
		pub core: CoreIndex,
		pub para_id: ParaId,
		pub kind: AssignmentKind,
		pub group_idx: GroupIndex,
	}

	impl From<CoreAssignment> for scheduler::CoreAssignment {
		fn from(old: CoreAssignment) -> Self {
			Self { core: old.core, para_id: old.para_id, kind: old.kind }
		}
	}
}

/// V1: Group index is dropped from the core assignment, it's explicitly computed during
/// candidates processing.
mod v1 {
	use super::*;

	pub fn migrate<T: Config>() -> Weight {
		let _ = <Pallet<T> as Store>::Scheduled::translate(
			|scheduled: Option<Vec<v0::CoreAssignment>>| {
				scheduled.map(|scheduled| {
					scheduled.into_iter().map(|old| scheduler::CoreAssignment::from(old)).collect()
				})
			},
		);

		T::DbWeight::get().reads_writes(1, 1)
	}
}
