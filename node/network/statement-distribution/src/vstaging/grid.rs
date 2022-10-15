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

//! Utilities for handling distribution of backed candidates along
//! the grid.

use polkadot_node_network_protocol::{grid_topology::SessionGridTopology, PeerId};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, GroupIndex, Hash, ValidatorIndex,
};

use std::collections::{HashMap, HashSet};

use bitvec::vec::BitVec;

use super::LOG_TARGET;

/// Our local view of a subset of the grid topology organized around a specific validator
/// group.
///
/// This tracks which authorities we expect to communicate with concerning
/// candidates from the group. This includes both the authorities we are
/// expected to send to as well as the authorities we expect to receive from.
///
/// In the case that this group is the group that we are locally assigned to,
/// the 'receiving' side will be empty.
struct GroupSubView {
	sending: HashSet<ValidatorIndex>,
	receiving: HashSet<ValidatorIndex>,
}

/// Our local view of the topology for a session, as it pertains to backed
/// candidate distribution.
struct SessionTopologyView {
	group_views: HashMap<GroupIndex, GroupSubView>,
}

/// Build a view of the topology for the session.
/// For groups that we are part of: we receive from nobody and send to our X/Y peers.
/// For groups that we are not part of: we receive from any validator in the group we share a slice with.
///    and send to the corresponding X/Y slice.
///    For any validators we don't share a slice with, we receive from the nodes
///    which share a slice with them.
fn build_session_topology(
	groups: &[Vec<ValidatorIndex>],
	topology: &SessionGridTopology,
	our_index: ValidatorIndex,
) -> SessionTopologyView {
	let mut view = SessionTopologyView { group_views: HashMap::new() };

	let our_neighbors = match topology.compute_grid_neighbors_for(our_index) {
		None => {
			gum::warn!(target: LOG_TARGET, ?our_index, "our index unrecognized in topology?");

			return view
		},
		Some(n) => n,
	};

	for (i, group) in groups.into_iter().enumerate() {
		let mut sub_view = GroupSubView { sending: HashSet::new(), receiving: HashSet::new() };

		if group.contains(&our_index) {
			sub_view.sending.extend(our_neighbors.validator_indices_x.iter().cloned());
			sub_view.sending.extend(our_neighbors.validator_indices_y.iter().cloned());
		} else {
			for &group_val in group {
				// If the validator shares a slice with us, we expect to
				// receive from them and send to our neighbors in the other
				// dimension.

				if our_neighbors.validator_indices_x.contains(&group_val) {
					sub_view.receiving.insert(group_val);
					sub_view.sending.extend(our_neighbors.validator_indices_y.iter().cloned());

					continue
				}

				if our_neighbors.validator_indices_y.contains(&group_val) {
					sub_view.receiving.insert(group_val);
					sub_view.sending.extend(our_neighbors.validator_indices_x.iter().cloned());

					continue
				}

				// If they don't share a slice with us, we don't send to anybody
				// but receive from any peers sharing a dimension with both of us.
				let their_neighbors = match topology.compute_grid_neighbors_for(group_val) {
					None => {
						gum::warn!(
							target: LOG_TARGET,
							index = ?group_val,
							"validator index unrecognized in topology?"
						);

						continue
					},
					Some(n) => n,
				};

				// their X, our Y
				for potential_link in &their_neighbors.validator_indices_x {
					if our_neighbors.validator_indices_y.contains(potential_link) {
						sub_view.receiving.insert(*potential_link);
						break // one max
					}
				}

				// their Y, our X
				for potential_link in &their_neighbors.validator_indices_y {
					if our_neighbors.validator_indices_x.contains(potential_link) {
						sub_view.receiving.insert(*potential_link);
						break // one max
					}
				}
			}
		}

		view.group_views.insert(GroupIndex(i as _), sub_view);
	}

	view
}

/// A tracker of knowledge from authorities within the grid for a
/// specific relay-parent.
struct PerRelayParentGridTracker {
	by_validator: HashMap<(ValidatorIndex, GroupIndex), Knowledge>,
}

struct ManifestSummary {
	claimed_parent_hash: Hash,
	seconded_in_group: BitVec<u8, bitvec::order::Lsb0>,
	validated_in_group: BitVec<u8, bitvec::order::Lsb0>,
}

struct Knowledge {
	manifests: HashMap<CandidateHash, ManifestSummary>,
	seconded_counts: Vec<usize>,
}

impl Knowledge {}

#[cfg(tests)]
mod tests {
	use super::*;

	// TODO [now]: test that grid topology views are set up correctly.
}
