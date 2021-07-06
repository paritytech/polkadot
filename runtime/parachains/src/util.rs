// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Utilities that don't belong to any particular module but may draw
//! on all modules.

use primitives::v1::{Id as ParaId, PersistedValidationData, ValidatorIndex};
use sp_std::vec::Vec;

use crate::{configuration, paras, hrmp};

/// Make the persisted validation data for a particular parachain, a specified relay-parent and it's
/// storage root.
///
/// This ties together the storage of several modules.
pub fn make_persisted_validation_data<T: paras::Config + hrmp::Config>(
	para_id: ParaId,
	relay_parent_number: T::BlockNumber,
	relay_parent_storage_root: T::Hash,
) -> Option<PersistedValidationData<T::Hash, T::BlockNumber>> {
	let config = <configuration::Module<T>>::config();

	Some(PersistedValidationData {
		parent_head: <paras::Pallet<T>>::para_head(&para_id)?,
		relay_parent_number,
		relay_parent_storage_root,
		max_pov_size: config.max_pov_size,
	})
}

/// Take the active subset of a set containing all validators.
pub fn take_active_subset<T: Clone>(active_validators: &[ValidatorIndex], set: &[T]) -> Vec<T> {
	let subset: Vec<_> = active_validators.iter()
		.filter_map(|i| set.get(i.0 as usize))
		.cloned()
		.collect();

	if subset.len() != active_validators.len() {
		log::warn!(
			target: "runtime::parachains",
			"Took active validators from set with wrong size",
		);
	}

	subset
}
