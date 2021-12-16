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

use crate::session_info::{Pallet, Store, Config};
use frame_support::{pallet_prelude::*, traits::StorageVersion, weights::Weight};
use frame_system::pallet_prelude::BlockNumberFor;

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
pub fn migrate_to_latest<T: Config>() -> Weight {
	let mut weight = 0;
	if StorageVersion::get::<Pallet<T>>() <= 1 {
		weight += migrate_to_v2::<T>();
		StorageVersion::new(2).put::<Pallet<T>>();
	}
	weight
}

mod v1 {
	use primitives::v1::*;
	use parity_scale_codec::{Decode, Encode};

	/// Information about validator sets of a session.
	#[derive(Encode, Decode, PartialEq, Default)]
	pub struct SessionInfo {
		pub validators: Vec<ValidatorId>,
		pub discovery_keys: Vec<AuthorityDiscoveryId>,
		pub assignment_keys: Vec<AssignmentId>,
		pub validator_groups: Vec<Vec<ValidatorIndex>>,
		pub n_cores: u32,
		pub zeroth_delay_tranche_width: u32,
		pub relay_vrf_modulo_samples: u32,
		pub n_delay_tranches: u32,
		pub no_show_slots: u32,
		pub needed_approvals: u32,
	}
}

pub fn migrate_to_v2<T: Config>() -> Weight {
	let translate_value = |old: v1::SessionInfo| -> Option<primitives::v2::SessionInfo> {
		Som(primitives::v2::SessionInfo {
			// new fields
			active_validator_indices: Vec::new(),
			// old fields
			validators: old.validators,
			discovery_keys: old.discovery_keys,
			assignment_keys: old.assignment_keys,
			validator_groups: old.validator_groups,
			n_cores: old.n_core,
			zeroth_delay_tranche_width: old.zeroth_delay_tranche_width,
			relay_vrf_modulo_samples: old.relay_vrf_modulo_samples,
			n_delay_tranches: old.n_delay_tranches,
			no_show_slots: old.no_show_slots,
			needed_approvals: old.needed_approvals,
		})
	};

	let mut vs = 0;

	<Pallet<T> as Store>::Sessions::translate_values(|v| {
		vs += 1;
		translate_value(v)
	});

	T::DbWeight::get().reads_writes(vs, vs)
}
