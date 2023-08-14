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

use crate::scheduler::{self, Config, Pallet, Scheduled};
use frame_support::traits::StorageVersion;

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

mod v0 {
	use super::*;
	use crate::scheduler::AssignmentKind;
	use frame_support::pallet_prelude::*;
	use parity_scale_codec::{Decode, Encode};
	use primitives::{CoreIndex, GroupIndex, Id as ParaId};

	#[derive(Clone, Encode, Decode, TypeInfo)]
	#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
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

	#[frame_support::storage_alias]
	pub(super) type Scheduled<T: Config> = StorageValue<Pallet<T>, Vec<CoreAssignment>, ValueQuery>;
}

/// V1: Group index is dropped from the core assignment, it's explicitly computed during
/// candidates processing.
pub mod v1 {
	use super::*;
	use frame_support::{pallet_prelude::*, traits::OnRuntimeUpgrade, weights::Weight};
	use sp_std::vec::Vec;

	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut weight: Weight = T::DbWeight::get().reads(1);

			if StorageVersion::get::<Pallet<T>>() < STORAGE_VERSION {
				log::info!(target: scheduler::LOG_TARGET, "Migrating scheduler storage to v1");
				weight = weight
					.saturating_add(migrate::<T>())
					.saturating_add(T::DbWeight::get().writes(1));
				STORAGE_VERSION.put::<Pallet<T>>();
			} else {
				log::info!(
					target: scheduler::LOG_TARGET,
					"Scheduler storage up to date - no need for migration"
				);
			}

			weight
		}
	}

	pub(super) fn migrate<T: Config>() -> Weight {
		let _ = Scheduled::<T>::translate(|scheduled: Option<Vec<v0::CoreAssignment>>| {
			scheduled.map(|scheduled| {
				scheduled.into_iter().map(|old| scheduler::CoreAssignment::from(old)).collect()
			})
		});

		T::DbWeight::get().reads_writes(1, 1)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		mock::{new_test_ext, Test},
		scheduler::{self, AssignmentKind},
	};
	use primitives::{CoreIndex, GroupIndex, Id as ParaId};

	#[test]
	fn test_migrate_to_v7() {
		let v0 = vec![
			v0::CoreAssignment {
				core: CoreIndex(0),
				para_id: ParaId::from(0),
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex(0),
			},
			v0::CoreAssignment {
				core: CoreIndex(1),
				para_id: ParaId::from(1),
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex(1),
			},
			v0::CoreAssignment {
				core: CoreIndex(2),
				para_id: ParaId::from(2),
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex(2),
			},
		];
		let expected_v1: Vec<_> =
			v0.clone().into_iter().map(scheduler::CoreAssignment::from).collect();

		new_test_ext(Default::default()).execute_with(|| {
			v0::Scheduled::<Test>::set(v0.clone());

			v1::migrate::<Test>();
			let v1 = Scheduled::<Test>::get();

			assert_eq!(v1, expected_v1);
		});
	}

	#[test]
	fn test_migrate_to_v7_no_entry() {
		new_test_ext(Default::default()).execute_with(|| {
			v0::Scheduled::<Test>::kill();

			v1::migrate::<Test>();
			let v1 = Scheduled::<Test>::get();

			assert!(v1.is_empty());
		});
	}
}
