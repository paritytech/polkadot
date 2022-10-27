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

use super::{groups::Groups, LOG_TARGET};

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
pub struct SessionTopologyView {
	group_views: HashMap<GroupIndex, GroupSubView>,
}

/// Build a view of the topology for the session.
/// For groups that we are part of: we receive from nobody and send to our X/Y peers.
/// For groups that we are not part of: we receive from any validator in the group we share a slice with.
///    and send to the corresponding X/Y slice.
///    For any validators we don't share a slice with, we receive from the nodes
///    which share a slice with them.
pub fn build_session_topology(
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

/// Actions that can be taken once affirming that a candidate is backed.
pub enum PostBackingAction {
	Acknowledge,
	Advertise,
}

/// A tracker of knowledge from authorities within the grid for the
/// entire session. This stores only data on manifests sent within a bounded
/// set of relay-parents.
#[derive(Default)]
pub struct PerRelayParentGridTracker {
	received: HashMap<ValidatorIndex, ReceivedManifests>,
	confirmed_backed: HashMap<CandidateHash, KnownBackedCandidate>,
	unconfirmed: HashMap<CandidateHash, Vec<(ValidatorIndex, GroupIndex)>>,
}

impl PerRelayParentGridTracker {
	/// Attempt to import a manifest advertised by a remote peer.
	///
	/// This checks whether the peer is allowed to send us manifests
	/// about this group at this relay-parent. This also does sanity
	/// checks on the format of the manifest and the amount of votes
	/// it contains. It has effects on the stored state only when successful.
	pub fn import_manifest(
		&mut self,
		session_topology: &SessionTopologyView,
		groups: &Groups,
		candidate_hash: CandidateHash,
		seconding_limit: usize,
		manifest: ManifestSummary,
		sender: ValidatorIndex,
	) -> Result<(), ManifestImportError> {
		let claimed_group_index = manifest.claimed_group_index;

		if session_topology
			.group_views
			.get(&manifest.claimed_group_index)
			.map_or(true, |g| !g.receiving.contains(&sender))
		{
			return Err(ManifestImportError::Disallowed)
		}

		let (group_size, backing_threshold) =
			match groups.get_size_and_backing_threshold(manifest.claimed_group_index) {
				Some(x) => x,
				None => return Err(ManifestImportError::Malformed),
			};

		if manifest.seconded_in_group.len() != group_size ||
			manifest.validated_in_group.len() != group_size
		{
			return Err(ManifestImportError::Malformed)
		}

		if manifest.seconded_in_group.count_ones() == 0 {
			return Err(ManifestImportError::Malformed)
		}

		// ensure votes are sufficient to back.
		let votes = manifest
			.seconded_in_group
			.iter()
			.by_vals()
			.zip(manifest.validated_in_group.iter().by_vals())
			.filter(|&(s, v)| s || v) // no double-counting
			.count();

		if votes < backing_threshold {
			return Err(ManifestImportError::Malformed)
		}

		let remote_knowledge = StatementFilter {
			seconded_in_group: manifest.seconded_in_group.clone(),
			validated_in_group: manifest.validated_in_group.clone(),
		};

		self.received.entry(sender).or_default().import_received(
			group_size,
			seconding_limit,
			candidate_hash,
			manifest,
		)?;

		if let Some(confirmed) = self.confirmed_backed.get_mut(&candidate_hash) {
			// TODO [now]: send statements they need?
			confirmed.note_remote_advertised(sender, remote_knowledge);
		} else {
			// received prevents conflicting manifests so this is max 1 per validator.
			self.unconfirmed
				.entry(candidate_hash)
				.or_default()
				.push((sender, claimed_group_index))
		}

		Ok(())
	}

	/// Add a new backed candidate to the tracker. This yields
	/// an iterator of validators which we should either advertise to
	/// or signal that we know the candidate.
	pub fn add_backed_candidate(
		&mut self,
		session_topology: &SessionTopologyView,
		candidate_hash: CandidateHash,
		group_index: GroupIndex,
		group_size: usize,
	) -> Vec<(ValidatorIndex, PostBackingAction)> {
		let mut actions = Vec::new();
		let c = match self.confirmed_backed.entry(candidate_hash) {
			Entry::Occupied(_) => return actions,
			Entry::Vacant(v) =>
				v.insert(KnownBackedCandidate { group_index, mutual_knowledge: HashMap::new() }),
		};

		// Populate the entry with previously unconfirmed manifests.
		for (v, claimed_group_index) in
			self.unconfirmed.remove(&candidate_hash).into_iter().flat_map(|x| x)
		{
			if claimed_group_index != group_index {
				// This is misbehavior, but is handled more comprehensively elsewhere
				continue
			}

			let statement_filter = self
				.received
				.get(&v)
				.and_then(|r| r.candidate_statement_filter(&candidate_hash))
				.expect("unconfirmed is only populated by validators who have sent manifest; qed");

			c.note_remote_advertised(v, statement_filter);
		}

		let group_topology = match session_topology.group_views.get(&group_index) {
			None => return actions,
			Some(g) => g,
		};

		// advertise onwards ad accept received advertisements

		for &v in &group_topology.sending {
			if c.should_advertise(v) {
				actions.push((v, PostBackingAction::Advertise))
			}
		}

		for &v in &group_topology.receiving {
			if c.can_local_acknowledge(v) {
				actions.push((v, PostBackingAction::Acknowledge))
			}
		}

		actions
	}

	/// Note that a backed candidate has been advertised to a
	/// given validator.
	pub fn note_advertised_to(
		&mut self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
		local_knowledge: StatementFilter,
	) {
		if let Some(c) = self.confirmed_backed.get_mut(&candidate_hash) {
			c.note_advertised_to(validator_index, local_knowledge);
		}
	}

	/// Provided a validator index, gives an iterator of candidate
	/// hashes which may be advertised to the validator and have not yet
	/// been.
	pub fn advertisements<'a>(
		&'a self,
		session_topology: &SessionTopologyView,
		validator_index: ValidatorIndex,
	) -> impl IntoIterator<Item = CandidateHash> + 'a {
		let allowed_groups: HashSet<_> = session_topology
			.group_views
			.iter()
			.filter(|(_, x)| x.sending.contains(&validator_index))
			.map(|(g, x)| *g)
			.collect();

		self.confirmed_backed
			.iter()
			.filter(move |(_, c)| allowed_groups.contains(c.group_index()))
			.filter(move |(_, c)| c.should_advertise(validator_index))
			.map(|(c_h, _)| *c_h)
	}

	/// Whether the given validator is allowed to acknowledge an advertisement
	/// via request.
	pub fn can_remote_acknowledge(
		&self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
	) -> bool {
		self.confirmed_backed
			.get(&candidate_hash)
			.map_or(false, |c| c.can_remote_acknowledge(validator_index))
	}

	/// Note that a validator peer we advertised a backed candidate to
	/// has acknowledged the candidate directly or by requesting it.
	pub fn note_remote_acknowledged(
		&mut self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
		remote_knowledge: StatementFilter,
	) {
		if let Some(c) = self.confirmed_backed.get_mut(&candidate_hash) {
			c.note_remote_acknowledged(validator_index, remote_knowledge);
		}
	}

	/// Whether we can acknowledge a remote's advertisement.
	pub fn can_local_acknowledge(
		&self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
	) -> bool {
		self.confirmed_backed
			.get(&candidate_hash)
			.map_or(false, |c| c.can_local_acknowledge(validator_index))
	}

	/// Indicate that we've acknowledged a remote's advertisement.
	pub fn note_local_acknowledged(
		&mut self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
		local_knowledge: StatementFilter,
	) {
		if let Some(c) = self.confirmed_backed.get_mut(&candidate_hash) {
			c.note_local_acknowledged(validator_index, local_knowledge);
		}
	}
}

/// A summary of a manifest being sent by a counterparty.
#[derive(Clone)]
pub struct ManifestSummary {
	/// The claimed parent head data hash of the candidate.
	pub claimed_parent_hash: Hash,
	/// The claimed group index assigned to the candidate.
	pub claimed_group_index: GroupIndex,
	/// A bitfield of validators in the group which seconded the
	/// candidate.
	pub seconded_in_group: BitVec<u8, Lsb0>,
	/// A bitfield of validators in the group which validated the
	/// candidate.
	pub validated_in_group: BitVec<u8, Lsb0>,
}

/// Errors in importing a manifest.
#[derive(Debug, Clone)]
pub enum ManifestImportError {
	/// The manifest conflicts with another, previously sent manifest.
	Conflicting,
	/// The manifest has overflowed beyond the limits of what the
	/// counterparty was allowed to send us.
	Overflow,
	/// The manifest claims insufficient attestations to achieve the backing
	/// threshold.
	Insufficient,
	/// The manifest is malformed.
	Malformed,
	/// The manifest was not allowed to be sent.
	Disallowed,
}

/// The knowledge we are awawre of counterparties having of manifests.
#[derive(Default)]
struct ReceivedManifests {
	received: HashMap<CandidateHash, ManifestSummary>,
	// group -> seconded counts.
	seconded_counts: HashMap<GroupIndex, Vec<usize>>,
}

impl ReceivedManifests {
	fn new() -> Self {
		ReceivedManifests { received: HashMap::new(), seconded_counts: HashMap::new() }
	}

	fn candidate_statement_filter(
		&self,
		candidate_hash: &CandidateHash,
	) -> Option<StatementFilter> {
		self.received.get(candidate_hash).map(|m| StatementFilter {
			seconded_in_group: m.seconded_in_group.clone(),
			validated_in_group: m.validated_in_group.clone(),
		})
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

#[derive(Clone, Copy)]
enum StatementKind {
	Seconded,
	Valid,
}

/// Bitfields indicating the statements that are known or undesired
/// about a candidate.
pub struct StatementFilter {
	/// Seconded statements. '1' is known or undesired.
	pub seconded_in_group: BitVec<u8, Lsb0>,
	/// Valid statements. '1' is known or undesired.
	pub validated_in_group: BitVec<u8, Lsb0>,
}

impl StatementFilter {
	/// Create a new filter with the given group size.
	pub fn new(group_size: usize) -> Self {
		StatementFilter {
			seconded_in_group: BitVec::repeat(false, group_size),
			validated_in_group: BitVec::repeat(false, group_size),
		}
	}

	fn contains(&self, index: usize, statement_kind: StatementKind) -> bool {
		match statement_kind {
			StatementKind::Seconded => self.seconded_in_group.get(index).map_or(false, |x| *x),
			StatementKind::Valid => self.validated_in_group.get(index).map_or(false, |x| *x),
		}
	}

	fn set(&mut self, index: usize, statement_kind: StatementKind) {
		let b = match statement_kind {
			StatementKind::Seconded => self.seconded_in_group.get_mut(index),
			StatementKind::Valid => self.validated_in_group.get_mut(index),
		};

		if let Some(mut b) = b {
			*b = true;
		}
	}
}

struct MutualKnowledge {
	// Knowledge they have about the candidate. `Some` only if they
	// have advertised or requested the candidate.
	remote_knowledge: Option<StatementFilter>,
	// Knowledge we have indicated to them about the candidate.
	// `Some` only if we have advertised or requested the candidate
	// from them.
	local_knowledge: Option<StatementFilter>,
}

// A utility struct for keeping track of metadata about candidates
// we have confirmed as having been backed.
struct KnownBackedCandidate {
	group_index: GroupIndex,
	mutual_knowledge: HashMap<ValidatorIndex, MutualKnowledge>,
}

impl KnownBackedCandidate {
	fn known_by(&self, validator: ValidatorIndex) -> bool {
		match self.mutual_knowledge.get(&validator) {
			None => false,
			Some(k) => k.remote_knowledge.is_some(),
		}
	}

	fn group_index(&self) -> &GroupIndex {
		&self.group_index
	}

	// should only be invoked for validators which are known
	// to be valid recipients of advertisement.
	fn should_advertise(&self, validator: ValidatorIndex) -> bool {
		self.mutual_knowledge
			.get(&validator)
			.map_or(true, |k| k.local_knowledge.is_none())
	}

	// is a no-op when either they or we have advertised.
	fn note_advertised_to(&mut self, validator: ValidatorIndex, local_knowledge: StatementFilter) {
		let k = self
			.mutual_knowledge
			.entry(validator)
			.or_insert_with(|| MutualKnowledge { remote_knowledge: None, local_knowledge: None });

		k.local_knowledge = Some(local_knowledge);
	}

	fn note_remote_advertised(
		&mut self,
		validator: ValidatorIndex,
		remote_knowledge: StatementFilter,
	) {
		let k = self
			.mutual_knowledge
			.entry(validator)
			.or_insert_with(|| MutualKnowledge { remote_knowledge: None, local_knowledge: None });

		k.remote_knowledge = Some(remote_knowledge);
	}

	// whether we are allowed to acknowledge or request a candidate from a remote validator.
	fn can_local_acknowledge(&self, validator: ValidatorIndex) -> bool {
		match self.mutual_knowledge.get(&validator) {
			None => false,
			Some(k) => k.remote_knowledge.is_some() && k.local_knowledge.is_none(),
		}
	}

	// whether a remote is allowed to acknowledge or request a candidate from us
	fn can_remote_acknowledge(&self, validator: ValidatorIndex) -> bool {
		match self.mutual_knowledge.get(&validator) {
			None => false,
			Some(k) => k.remote_knowledge.is_none() && k.local_knowledge.is_some(),
		}
	}

	fn note_local_acknowledged(
		&mut self,
		validator: ValidatorIndex,
		local_knowledge: StatementFilter,
	) {
		if let Some(ref mut k) = self.mutual_knowledge.get_mut(&validator) {
			k.local_knowledge = Some(local_knowledge);
			// TODO [now]: return something for sending statements they need.
		}
	}

	fn note_remote_acknowledged(
		&mut self,
		validator: ValidatorIndex,
		remote_knowledge: StatementFilter,
	) {
		if let Some(ref mut k) = self.mutual_knowledge.get_mut(&validator) {
			k.remote_knowledge = Some(remote_knowledge);
			// TODO [now]: return something for sending statements they need.
		}
	}

	fn can_send_direct_statement_to(
		&self,
		validator: ValidatorIndex,
		statement_index_in_group: usize,
		statement_kind: StatementKind,
	) -> bool {
		match self.mutual_knowledge.get(&validator) {
			Some(MutualKnowledge { remote_knowledge: Some(r), local_knowledge: Some(_) }) =>
				!r.contains(statement_index_in_group, statement_kind),
			_ => false,
		}
	}

	fn can_receive_direct_statement_from(
		&self,
		validator: ValidatorIndex,
		statement_index_in_group: usize,
		statement_kind: StatementKind,
	) -> bool {
		match self.mutual_knowledge.get(&validator) {
			Some(MutualKnowledge { remote_knowledge: Some(_), local_knowledge: Some(l) }) =>
				!l.contains(statement_index_in_group, statement_kind),
			_ => false,
		}
	}

	fn note_sent_or_received_direct_statement(
		&mut self,
		validator: ValidatorIndex,
		statement_index_in_group: usize,
		statement_kind: StatementKind,
	) {
		if let Some(k) = self.mutual_knowledge.get_mut(&validator) {
			if let (Some(r), Some(l)) = (k.remote_knowledge.as_mut(), k.local_knowledge.as_mut()) {
				r.set(statement_index_in_group, statement_kind);
				l.set(statement_index_in_group, statement_kind);
			}
		}
	}
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
		let mut knowledge = ReceivedManifests::default();

		let expected_manifest_summary = ManifestSummary {
			claimed_parent_hash: Hash::repeat_byte(2),
			claimed_group_index: GroupIndex(0),
			seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
			validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
		};

		knowledge
			.import_received(
				3,
				2,
				CandidateHash(Hash::repeat_byte(1)),
				expected_manifest_summary.clone(),
			)
			.unwrap();

		// conflicting group

		let mut s = expected_manifest_summary.clone();
		s.claimed_group_index = GroupIndex(1);
		assert_matches!(
			knowledge.import_received(3, 2, CandidateHash(Hash::repeat_byte(1)), s,),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting parent hash

		let mut s = expected_manifest_summary.clone();
		s.claimed_parent_hash = Hash::repeat_byte(3);
		assert_matches!(
			knowledge.import_received(3, 2, CandidateHash(Hash::repeat_byte(1)), s,),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting seconded statements bitfield

		let mut s = expected_manifest_summary.clone();
		s.seconded_in_group = bitvec::bitvec![u8, Lsb0; 0, 1, 0];
		assert_matches!(
			knowledge.import_received(3, 2, CandidateHash(Hash::repeat_byte(1)), s,),
			Err(ManifestImportError::Conflicting)
		);

		// conflicting valid statements bitfield

		let mut s = expected_manifest_summary.clone();
		s.validated_in_group = bitvec::bitvec![u8, Lsb0; 0, 1, 0];
		assert_matches!(
			knowledge.import_received(3, 2, CandidateHash(Hash::repeat_byte(1)), s,),
			Err(ManifestImportError::Conflicting)
		);
	}

	#[test]
	fn reject_overflowing_manifests() {
		let mut knowledge = ReceivedManifests::default();
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

	#[test]
	fn reject_disallowed_manifest() {
		let mut tracker = PerRelayParentGridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: vec![ValidatorIndex(0)].into_iter().collect(),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]],
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		assert_eq!(groups.get_size_and_backing_threshold(GroupIndex(0)), Some((3, 2)),);

		// Known group, disallowed receiving validator.

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
				},
				ValidatorIndex(1),
			),
			Err(ManifestImportError::Disallowed)
		);

		// Unknown group

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(1),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Disallowed)
		);
	}

	#[test]
	fn reject_malformed_wrong_group_size() {
		let mut tracker = PerRelayParentGridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: vec![ValidatorIndex(0)].into_iter().collect(),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]],
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		assert_eq!(groups.get_size_and_backing_threshold(GroupIndex(0)), Some((3, 2)),);

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1, 0],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);
	}

	#[test]
	fn reject_malformed_no_seconders() {
		let mut tracker = PerRelayParentGridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: vec![ValidatorIndex(0)].into_iter().collect(),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]],
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		assert_eq!(groups.get_size_and_backing_threshold(GroupIndex(0)), Some((3, 2)),);

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);
	}

	#[test]
	fn reject_malformed_below_threshold() {
		let mut tracker = PerRelayParentGridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: vec![ValidatorIndex(0)].into_iter().collect(),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]],
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		assert_eq!(groups.get_size_and_backing_threshold(GroupIndex(0)), Some((3, 2)),);

		// only one vote

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);

		// seconding + validating still not enough to reach '2' threshold

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 1],
				},
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);

		// finally good.

		assert_matches!(
			tracker.import_manifest(
				&session_topology,
				&groups,
				candidate_hash,
				3,
				ManifestSummary {
					claimed_parent_hash: Hash::repeat_byte(0),
					claimed_group_index: GroupIndex(0),
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				},
				ValidatorIndex(0),
			),
			Ok(())
		);
	}
}
