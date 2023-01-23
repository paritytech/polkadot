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

//! A utility for tracking groups and their members within a session.

use polkadot_node_primitives::minimum_votes;
use polkadot_primitives::vstaging::{AuthorityDiscoveryId, GroupIndex, IndexedVec, ValidatorIndex};

use std::collections::HashMap;

/// Validator groups within a session, plus some helpful indexing for
/// looking up groups by validator indices or authority discovery ID.
#[derive(Debug, Clone)]
pub struct Groups {
	groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	by_validator_index: HashMap<ValidatorIndex, GroupIndex>,
	by_discovery_key: HashMap<AuthorityDiscoveryId, GroupIndex>,
}

impl Groups {
	/// Create a new [`Groups`] tracker with the groups and discovery keys
	/// from the session.
	pub fn new(
		groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
		discovery_keys: &[AuthorityDiscoveryId],
	) -> Self {
		let mut by_validator_index = HashMap::new();
		let mut by_discovery_key = HashMap::new();

		for (i, group) in groups.iter().enumerate() {
			let index = GroupIndex(i as _);
			for v in group {
				by_validator_index.insert(*v, index);
				if let Some(discovery_key) = discovery_keys.get(v.0 as usize) {
					// GIGO: malformed session data leads to incomplete index.
					by_discovery_key.insert(discovery_key.clone(), index);
				}
			}
		}

		Groups { groups, by_validator_index, by_discovery_key }
	}

	/// Access all the underlying groups.
	pub fn all(&self) -> &IndexedVec<GroupIndex, Vec<ValidatorIndex>> {
		&self.groups
	}

	/// Get the underlying group validators by group index.
	pub fn get(&self, group_index: GroupIndex) -> Option<&[ValidatorIndex]> {
		self.groups.get(group_index).map(|x| &x[..])
	}

	/// Get the backing group size and backing threshold.
	pub fn get_size_and_backing_threshold(
		&self,
		group_index: GroupIndex,
	) -> Option<(usize, usize)> {
		self.get(group_index).map(|g| (g.len(), minimum_votes(g.len())))
	}

	/// Get the group index for a validator by index.
	pub fn by_validator_index(&self, validator_index: ValidatorIndex) -> Option<GroupIndex> {
		self.by_validator_index.get(&validator_index).map(|x| *x)
	}

	/// Get the group index for a validator by its discovery key.
	pub fn by_discovery_key(&self, discovery_key: AuthorityDiscoveryId) -> Option<GroupIndex> {
		self.by_discovery_key.get(&discovery_key).map(|x| *x)
	}
}
