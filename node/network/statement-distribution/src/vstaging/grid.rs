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

use std::collections::{
	hash_map::{Entry, HashMap},
	HashSet,
};

use bitvec::{order::Lsb0, slice::BitSlice, vec::BitVec};

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
#[derive(PartialEq)]
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
	our_index: Option<ValidatorIndex>,
) -> SessionTopologyView {
	let mut view = SessionTopologyView { group_views: HashMap::new() };

	let our_index = match our_index {
		None => return view,
		Some(i) => i,
	};

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
					sub_view.sending.extend(
						our_neighbors
							.validator_indices_y
							.iter()
							.filter(|v| !group.contains(v))
							.cloned(),
					);

					continue
				}

				if our_neighbors.validator_indices_y.contains(&group_val) {
					sub_view.receiving.insert(group_val);
					sub_view.sending.extend(
						our_neighbors
							.validator_indices_x
							.iter()
							.filter(|v| !group.contains(v))
							.cloned(),
					);

					continue
				}

				// If they don't share a slice with us, we don't send to anybody
				// but receive from any peers sharing a dimension with both of us
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
	received: HashMap<ValidatorIndex, CounterPartyManifestKnowledge>,
}

struct ManifestSummary {
	claimed_parent_hash: Hash,
	claimed_group_index: GroupIndex,
	seconded_in_group: BitVec<u8, Lsb0>,
	validated_in_group: BitVec<u8, Lsb0>,
}

#[derive(Debug, Clone)]
enum ManifestImportError {
	// The manifest conflicts with another, previously sent manifest.
	Conflicting,
	// The manifest has overflowed beyond the limits of what the
	// counterparty was allowed to send us.
	Overflow,
}

/// The knowledge we are awawre of counterparties having of manifests.
#[derive(Default)]
struct CounterPartyManifestKnowledge {
	received: HashMap<CandidateHash, ManifestSummary>,
	seconded_counts: HashMap<GroupIndex, Vec<usize>>,
}

impl CounterPartyManifestKnowledge {
	fn new() -> Self {
		CounterPartyManifestKnowledge { received: HashMap::new(), seconded_counts: HashMap::new() }
	}

	/// Attempt to import a received manifest from a counterparty.
	///
	/// This will reject manifests which are either duplicate, conflicting,
	/// or imply an irrational amount of `Seconded` statements.
	///
	/// This assumes that the manifest has already been checked for
	/// validity - i.e. that the bitvecs match the claimed group in size
	/// and that that the manifest includes at least one `Seconded`
	/// attestation and includes enough attestations for the candidate
	/// to be backed.
	///
	/// This also should only be invoked when we are intended to track
	/// the knowledge of this peer as determined by the [`SessionTopology`].
	fn import_received(
		&mut self,
		group_size: usize,
		seconding_limit: usize,
		candidate_hash: CandidateHash,
		manifest_summary: ManifestSummary,
	) -> Result<(), ManifestImportError> {
		match self.received.entry(candidate_hash) {
			Entry::Occupied(mut e) => {
				// occupied entry.

				// filter out clearly conflicting data.
				{
					let prev = e.get();
					if prev.claimed_group_index != manifest_summary.claimed_group_index {
						return Err(ManifestImportError::Conflicting)
					}

					if prev.claimed_parent_hash != manifest_summary.claimed_parent_hash {
						return Err(ManifestImportError::Conflicting)
					}

					if !manifest_summary.seconded_in_group.contains(&prev.seconded_in_group) {
						return Err(ManifestImportError::Conflicting)
					}

					if !manifest_summary.validated_in_group.contains(&prev.validated_in_group) {
						return Err(ManifestImportError::Conflicting)
					}

					let mut fresh_seconded = manifest_summary.seconded_in_group.clone();
					fresh_seconded |= &prev.seconded_in_group;

					let mut fresh_validated = manifest_summary.validated_in_group.clone();
					fresh_validated |= &prev.validated_in_group;

					let within_limits = updating_ensure_within_seconding_limit(
						&mut self.seconded_counts,
						manifest_summary.claimed_group_index,
						group_size,
						seconding_limit,
						&*fresh_seconded,
					);

					if !within_limits {
						return Err(ManifestImportError::Overflow)
					}
				}

				// All checks passed. Overwrite: guaranteed to be
				// superset.
				*e.get_mut() = manifest_summary;
				Ok(())
			},
			Entry::Vacant(e) => {
				let within_limits = updating_ensure_within_seconding_limit(
					&mut self.seconded_counts,
					manifest_summary.claimed_group_index,
					group_size,
					seconding_limit,
					&*manifest_summary.seconded_in_group,
				);

				if within_limits {
					e.insert(manifest_summary);
					Ok(())
				} else {
					Err(ManifestImportError::Overflow)
				}
			},
		}
	}
}

// updates validator-seconded records but only if the new statements
// are OK. returns `true` if alright and `false` otherwise.
fn updating_ensure_within_seconding_limit(
	seconded_counts: &mut HashMap<GroupIndex, Vec<usize>>,
	group_index: GroupIndex,
	group_size: usize,
	seconding_limit: usize,
	new_seconded: &BitSlice<u8, Lsb0>,
) -> bool {
	if seconding_limit == 0 {
		return false
	}

	// due to the check above, if this was non-existent this function will
	// always return `true`.
	let counts = seconded_counts.entry(group_index).or_insert_with(|| vec![0; group_size]);

	for i in new_seconded.iter_ones() {
		if counts[i] == seconding_limit {
			return false
		}
	}

	for i in new_seconded.iter_ones() {
		counts[i] += 1;
	}

	true
}

#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use polkadot_node_network_protocol::grid_topology::TopologyPeerInfo;
	use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
	use sp_core::crypto::Pair as PairT;

	#[test]
	fn topology_empty_for_no_index() {
		let base_topology = SessionGridTopology::new(
			vec![0, 1, 2],
			vec![
				TopologyPeerInfo {
					peer_ids: Vec::new(),
					validator_index: ValidatorIndex(0),
					discovery_id: AuthorityDiscoveryPair::generate().0.public(),
				},
				TopologyPeerInfo {
					peer_ids: Vec::new(),
					validator_index: ValidatorIndex(1),
					discovery_id: AuthorityDiscoveryPair::generate().0.public(),
				},
				TopologyPeerInfo {
					peer_ids: Vec::new(),
					validator_index: ValidatorIndex(2),
					discovery_id: AuthorityDiscoveryPair::generate().0.public(),
				},
			],
		);

		let t = build_session_topology(
			&[vec![ValidatorIndex(0)], vec![ValidatorIndex(1)], vec![ValidatorIndex(2)]],
			&base_topology,
			None,
		);

		assert!(t.group_views.is_empty());
	}

	#[test]
	fn topology_setup() {
		let base_topology = SessionGridTopology::new(
			(0..9).collect(),
			(0..9)
				.map(|i| TopologyPeerInfo {
					peer_ids: Vec::new(),
					validator_index: ValidatorIndex(i),
					discovery_id: AuthorityDiscoveryPair::generate().0.public(),
				})
				.collect(),
		);

		let t = build_session_topology(
			&[
				vec![ValidatorIndex(0), ValidatorIndex(3), ValidatorIndex(6)],
				vec![ValidatorIndex(4), ValidatorIndex(2), ValidatorIndex(7)],
				vec![ValidatorIndex(8), ValidatorIndex(5), ValidatorIndex(1)],
			],
			&base_topology,
			Some(ValidatorIndex(0)),
		);

		assert_eq!(t.group_views.len(), 3);

		// 0 1 2
		// 3 4 5
		// 6 7 8

		// our group: we send to all row/column neighbors and receive nothing
		assert_eq!(
			t.group_views.get(&GroupIndex(0)).unwrap().sending,
			vec![1, 2, 3, 6].into_iter().map(ValidatorIndex).collect::<HashSet<_>>(),
		);
		assert_eq!(t.group_views.get(&GroupIndex(0)).unwrap().receiving, HashSet::new(),);

		// we share a row with '2' and have indirect connections to '4' and '7'.

		assert_eq!(
			t.group_views.get(&GroupIndex(1)).unwrap().sending,
			vec![3, 6].into_iter().map(ValidatorIndex).collect::<HashSet<_>>(),
		);
		assert_eq!(
			t.group_views.get(&GroupIndex(1)).unwrap().receiving,
			vec![1, 2, 3, 6].into_iter().map(ValidatorIndex).collect::<HashSet<_>>(),
		);

		// we share a row with '1' and have indirect connections to '5' and '8'.

		assert_eq!(
			t.group_views.get(&GroupIndex(2)).unwrap().sending,
			vec![3, 6].into_iter().map(ValidatorIndex).collect::<HashSet<_>>(),
		);
		assert_eq!(
			t.group_views.get(&GroupIndex(2)).unwrap().receiving,
			vec![1, 2, 3, 6].into_iter().map(ValidatorIndex).collect::<HashSet<_>>(),
		);
	}

	#[test]
	fn knowledge_rejects_conflicting_manifest() {
		let mut knowledge = CounterPartyManifestKnowledge::new();
		knowledge
			.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(2),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			)
			.unwrap();

		// conflicting group

		assert_matches!(
			knowledge.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(2),
					claimed_group_index: GroupIndex(1),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting parent hash

		assert_matches!(
			knowledge.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(3),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting seconded statements bitfield

		assert_matches!(
			knowledge.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(2),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting valid statements bitfield

		assert_matches!(
			knowledge.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(2),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				},
			),
			Err(ManifestImportError::Conflicting)
		);
	}

	#[test]
	fn reject_overflowing_manifests() {
		let mut knowledge = CounterPartyManifestKnowledge::new();
		knowledge
			.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0xA),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			)
			.unwrap();

		knowledge
			.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(2)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0xB),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			)
			.unwrap();

		assert_matches!(
			knowledge.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(3)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0xC),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			),
			Err(ManifestImportError::Overflow)
		);

		knowledge
			.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(3)),
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0xC),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				},
			)
			.unwrap();
	}
}
