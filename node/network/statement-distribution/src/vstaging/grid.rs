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
	AuthorityDiscoveryId, CandidateHash, CompactStatement, GroupIndex, Hash, ValidatorIndex,
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
#[derive(Debug, PartialEq)]
struct GroupSubView {
	sending: HashSet<ValidatorIndex>,
	receiving: HashSet<ValidatorIndex>,
}

/// Our local view of the topology for a session, as it pertains to backed
/// candidate distribution.
pub struct SessionTopologyView {
	group_views: HashMap<GroupIndex, GroupSubView>,
}

impl SessionTopologyView {
	/// Returns an iterator over all validator indices from the group who are allowed to
	/// send us manifests.
	pub fn iter_group_senders<'a>(
		&'a self,
		group: GroupIndex,
	) -> impl Iterator<Item = ValidatorIndex> + 'a {
		self.group_views
			.get(&group)
			.into_iter()
			.flat_map(|sub| sub.sending.iter().cloned())
	}
}

/// Build a view of the topology for the session.
/// For groups that we are part of: we receive from nobody and send to our X/Y peers.
/// For groups that we are not part of: we receive from any validator in the group we share a slice with.
///    and send to the corresponding X/Y slice.
///    For any validators we don't share a slice with, we receive from the nodes
///    which share a slice with them.
pub fn build_session_topology<'a>(
	groups: impl IntoIterator<Item = &'a Vec<ValidatorIndex>>,
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

/// The kind of backed candidate manifest we should send to a remote peer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ManifestKind {
	/// Full manifests contain information about the candidate and should be sent
	/// to peers which aren't guaranteed to have the candidate already.
	Full,
	/// Acknowledgement manifests omit information which is implicit in the candidate
	/// itself, and should be sent to peers which are guaranteed to have the candidate
	/// already.
	Acknowledgement,
}

/// A tracker of knowledge from authorities within the grid for a particular
/// relay-parent.
#[derive(Default)]
pub struct GridTracker {
	received: HashMap<ValidatorIndex, ReceivedManifests>,
	confirmed_backed: HashMap<CandidateHash, KnownBackedCandidate>,
	unconfirmed: HashMap<CandidateHash, Vec<(ValidatorIndex, GroupIndex)>>,
	pending_manifests: HashMap<ValidatorIndex, HashMap<CandidateHash, ManifestKind>>,

	// maps target to (originator, statement) pairs.
	pending_statements: HashMap<ValidatorIndex, HashSet<(ValidatorIndex, CompactStatement)>>,
}

impl GridTracker {
	/// Attempt to import a manifest advertised by a remote peer.
	///
	/// This checks whether the peer is allowed to send us manifests
	/// about this group at this relay-parent. This also does sanity
	/// checks on the format of the manifest and the amount of votes
	/// it contains. It has effects on the stored state only when successful.
	///
	/// This returns a `bool` on success, which if true indicates that an acknowledgement is
	/// to be sent in response to the received manifest. This only occurs when the
	/// candidate is already known to be confirmed and backed.
	pub fn import_manifest(
		&mut self,
		session_topology: &SessionTopologyView,
		groups: &Groups,
		candidate_hash: CandidateHash,
		seconding_limit: usize,
		manifest: ManifestSummary,
		kind: ManifestKind,
		sender: ValidatorIndex,
	) -> Result<bool, ManifestImportError> {
		let claimed_group_index = manifest.claimed_group_index;

		let group_topology = match session_topology.group_views.get(&manifest.claimed_group_index) {
			None => return Err(ManifestImportError::Disallowed),
			Some(g) => g,
		};

		let receiving_from = group_topology.receiving.contains(&sender);
		let sending_to = group_topology.sending.contains(&sender);
		let manifest_allowed = match kind {
			// Peers can send manifests _if_:
			//   * They are in the receiving set for the group AND the manifest is full OR
			//   * They are in the sending set for the group AND we have sent them
			//     a manifest AND the received manifest is partial.
			ManifestKind::Full => receiving_from,
			ManifestKind::Acknowledgement =>
				sending_to &&
					self.confirmed_backed
						.get(&candidate_hash)
						.map_or(false, |c| c.has_sent_manifest_to(sender)),
		};

		if !manifest_allowed {
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

		let mut ack = false;
		if let Some(confirmed) = self.confirmed_backed.get_mut(&candidate_hash) {
			if receiving_from && !confirmed.has_sent_manifest_to(sender) {
				// due to checks above, the manifest `kind` is guaranteed to be `Full`
				self.pending_manifests
					.entry(sender)
					.or_default()
					.insert(candidate_hash, ManifestKind::Acknowledgement);

				ack = true;
			}

			// add all statements in local_knowledge & !remote_knowledge
			// to `pending_statements` for this validator.
			confirmed.manifest_received_from(sender, remote_knowledge);
			if let Some(pending_statements) = confirmed.pending_statements(sender) {
				self.pending_statements.entry(sender).or_default().extend(
					decompose_statement_filter(
						groups,
						claimed_group_index,
						candidate_hash,
						&pending_statements,
					),
				);
			}
		} else {
			// received prevents conflicting manifests so this is max 1 per validator.
			self.unconfirmed
				.entry(candidate_hash)
				.or_default()
				.push((sender, claimed_group_index))
		}

		Ok(ack)
	}

	/// Add a new backed candidate to the tracker. This yields
	/// an iterator of validators which we should either advertise to
	/// or signal that we know the candidate, along with the corresponding
	/// type of manifest we should send.
	pub fn add_backed_candidate(
		&mut self,
		session_topology: &SessionTopologyView,
		candidate_hash: CandidateHash,
		group_index: GroupIndex,
		group_size: usize,
		local_knowledge: StatementFilter,
	) -> Vec<(ValidatorIndex, ManifestKind)> {
		let c = match self.confirmed_backed.entry(candidate_hash) {
			Entry::Occupied(_) => return Vec::new(),
			Entry::Vacant(v) => v.insert(KnownBackedCandidate {
				group_index,
				mutual_knowledge: HashMap::new(),
				local_knowledge,
			}),
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

			// No need to send direct statements, because our local knowledge is `None`
			c.manifest_received_from(v, statement_filter);
		}

		let group_topology = match session_topology.group_views.get(&group_index) {
			None => return Vec::new(),
			Some(g) => g,
		};

		// advertise onwards and accept received advertisements

		let sending_group_manifests =
			group_topology.sending.iter().map(|v| (*v, ManifestKind::Full));

		let receiving_group_manifests = group_topology.receiving.iter().filter_map(|v| {
			if c.has_received_manifest_from(*v) {
				Some((*v, ManifestKind::Acknowledgement))
			} else {
				None
			}
		});

		// Note that order is important: if a validator is part of both the sending
		// and receiving groups, we may overwrite a `Full` manifest with a `Acknowledgement`
		// one.
		for (v, manifest_mode) in sending_group_manifests.chain(receiving_group_manifests) {
			self.pending_manifests
				.entry(v)
				.or_default()
				.insert(candidate_hash, manifest_mode);
		}

		self.pending_manifests
			.iter()
			.filter_map(|(v, x)| x.get(&candidate_hash).map(|k| (*v, *k)))
			.collect()
	}

	/// Note that a backed candidate has been advertised to a
	/// given validator.
	pub fn manifest_sent_to(
		&mut self,
		groups: &Groups,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
		local_knowledge: StatementFilter,
	) {
		if let Some(c) = self.confirmed_backed.get_mut(&candidate_hash) {
			c.manifest_sent_to(validator_index, local_knowledge);

			if let Some(pending_statements) = c.pending_statements(validator_index) {
				self.pending_statements.entry(validator_index).or_default().extend(
					decompose_statement_filter(
						groups,
						c.group_index,
						candidate_hash,
						&pending_statements,
					),
				);
			}
		}

		if let Some(x) = self.pending_manifests.get_mut(&validator_index) {
			x.remove(&candidate_hash);
		}
	}

	/// Whether we should send a manifest about a specific candidate to a validator,
	/// and which kind of manifest.
	pub fn is_manifest_pending_for(
		&self,
		validator_index: ValidatorIndex,
		candidate_hash: &CandidateHash,
	) -> Option<ManifestKind> {
		self.pending_manifests
			.get(&validator_index)
			.and_then(|x| x.get(&candidate_hash))
			.map(|x| *x)
	}

	/// Returns a vector of all candidates pending manifests for the specific validator, and
	/// the type of manifest we should send.
	pub fn pending_manifests_for(
		&self,
		validator_index: ValidatorIndex,
	) -> Vec<(CandidateHash, ManifestKind)> {
		self.pending_manifests
			.get(&validator_index)
			.into_iter()
			.flat_map(|pending| pending.iter().map(|(c, m)| (*c, *m)))
			.collect()
	}

	/// Returns a statement filter indicating statements that a given peer
	/// is awaiting concerning the given candidate, constrained by the statements
	/// we have ourselves.
	pub fn pending_statements_for(
		&self,
		validator_index: ValidatorIndex,
		candidate_hash: CandidateHash,
	) -> Option<StatementFilter> {
		self.confirmed_backed
			.get(&candidate_hash)
			.and_then(|x| x.pending_statements(validator_index))
	}

	/// Returns a vector of all pending statements to the validator, sorted with
	/// `Seconded` statements at the front.
	///
	/// Statements are in the form `(Originator, Statement Kind)`.
	pub fn all_pending_statements_for(
		&self,
		validator_index: ValidatorIndex,
	) -> Vec<(ValidatorIndex, CompactStatement)> {
		let mut v = self
			.pending_statements
			.get(&validator_index)
			.map(|x| x.iter().cloned().collect())
			.unwrap_or(Vec::new());

		v.sort_by_key(|(_, s)| match s {
			CompactStatement::Seconded(_) => 0u32,
			CompactStatement::Valid(_) => 1u32,
		});

		v
	}

	/// Which validators we could request the fully attested candidates from.
	/// If the candidate is already confirmed, then this will return an empty
	/// set.
	pub fn validators_to_request(
		&self,
		candidate_hash: CandidateHash,
		group_index: GroupIndex,
	) -> Vec<ValidatorIndex> {
		let mut validators = Vec::new();
		if let Some(unconfirmed) = self.unconfirmed.get(&candidate_hash) {
			for (v, g) in unconfirmed {
				if g == &group_index {
					validators.push(*v);
				}
			}
		}
		validators
	}

	/// Determine the validators which can send a statement to us by direct broadcast.
	pub fn direct_statement_providers(
		&self,
		groups: &Groups,
		originator: ValidatorIndex,
		statement: &CompactStatement,
	) -> Vec<ValidatorIndex> {
		let (g, c_h, kind, in_group) =
			match extract_statement_and_group_info(groups, originator, statement) {
				None => return Vec::new(),
				Some(x) => x,
			};

		self.confirmed_backed
			.get(&c_h)
			.map(|k| k.direct_statement_senders(g, in_group, kind))
			.unwrap_or_default()
	}

	/// Determine the validators which can receive a statement from us by direct
	/// broadcast.
	pub fn direct_statement_targets(
		&self,
		groups: &Groups,
		originator: ValidatorIndex,
		statement: &CompactStatement,
	) -> Vec<ValidatorIndex> {
		let (g, c_h, kind, in_group) =
			match extract_statement_and_group_info(groups, originator, statement) {
				None => return Vec::new(),
				Some(x) => x,
			};

		self.confirmed_backed
			.get(&c_h)
			.map(|k| k.direct_statement_recipients(g, in_group, kind))
			.unwrap_or_default()
	}

	/// Note that we have learned about a statement. This will update
	/// `pending_statements_for` for any relevant validators if actually
	/// fresh.
	pub fn learned_fresh_statement(
		&mut self,
		groups: &Groups,
		session_topology: &SessionTopologyView,
		originator: ValidatorIndex,
		statement: &CompactStatement,
	) {
		let (g, c_h, kind, in_group) =
			match extract_statement_and_group_info(groups, originator, statement) {
				None => return,
				Some(x) => x,
			};

		let known = match self.confirmed_backed.get_mut(&c_h) {
			None => return,
			Some(x) => x,
		};

		if !known.note_fresh_statement(in_group, kind) {
			return
		}

		// Add to `pending_statements` for all validators we communicate with
		// who have exchanged manifests.
		let all_group_validators = session_topology
			.group_views
			.get(&g)
			.into_iter()
			.flat_map(|g| g.sending.iter().chain(g.receiving.iter()));

		for v in all_group_validators {
			if known.is_pending_statement(*v, in_group, kind) {
				self.pending_statements
					.entry(*v)
					.or_default()
					.insert((originator, statement.clone()));
			}
		}
	}

	/// Note that a direct statement about a given candidate was sent to or
	/// received from the given validator.
	pub fn sent_or_received_direct_statement(
		&mut self,
		groups: &Groups,
		originator: ValidatorIndex,
		counterparty: ValidatorIndex,
		statement: &CompactStatement,
	) {
		if let Some((g, c_h, kind, in_group)) =
			extract_statement_and_group_info(groups, originator, statement)
		{
			if let Some(known) = self.confirmed_backed.get_mut(&c_h) {
				known.sent_or_received_direct_statement(counterparty, in_group, kind);

				if let Some(pending) = self.pending_statements.get_mut(&counterparty) {
					pending.remove(&(originator, statement.clone()));
				}
			}
		}
	}
}

fn extract_statement_and_group_info(
	groups: &Groups,
	originator: ValidatorIndex,
	statement: &CompactStatement,
) -> Option<(GroupIndex, CandidateHash, StatementKind, usize)> {
	let (statement_kind, candidate_hash) = match statement {
		CompactStatement::Seconded(h) => (StatementKind::Seconded, h),
		CompactStatement::Valid(h) => (StatementKind::Valid, h),
	};

	let group = match groups.by_validator_index(originator) {
		None => return None,
		Some(g) => g,
	};

	let index_in_group = groups.get(group)?.iter().position(|v| v == &originator)?;

	Some((group, *candidate_hash, statement_kind, index_in_group))
}

fn decompose_statement_filter<'a>(
	groups: &'a Groups,
	group_index: GroupIndex,
	candidate_hash: CandidateHash,
	statement_filter: &'a StatementFilter,
) -> impl Iterator<Item = (ValidatorIndex, CompactStatement)> + 'a {
	groups.get(group_index).into_iter().flat_map(move |g| {
		let s = statement_filter
			.seconded_in_group
			.iter_ones()
			.map(|i| g[i].clone())
			.map(move |i| (i, CompactStatement::Seconded(candidate_hash)));

		let v = statement_filter
			.validated_in_group
			.iter_ones()
			.map(|i| g[i].clone())
			.map(move |i| (i, CompactStatement::Valid(candidate_hash)));

		s.chain(v)
	})
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

#[derive(Debug, Clone, Copy)]
enum StatementKind {
	Seconded,
	Valid,
}

/// Bitfields indicating the statements that are known or undesired
/// about a candidate.
#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
struct KnownBackedCandidate {
	group_index: GroupIndex,
	local_knowledge: StatementFilter,
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

	fn has_received_manifest_from(&self, validator: ValidatorIndex) -> bool {
		self.mutual_knowledge
			.get(&validator)
			.map_or(false, |k| k.remote_knowledge.is_some())
	}

	fn has_sent_manifest_to(&self, validator: ValidatorIndex) -> bool {
		self.mutual_knowledge
			.get(&validator)
			.map_or(false, |k| k.local_knowledge.is_some())
	}

	fn manifest_sent_to(&mut self, validator: ValidatorIndex, local_knowledge: StatementFilter) {
		let k = self
			.mutual_knowledge
			.entry(validator)
			.or_insert_with(|| MutualKnowledge { remote_knowledge: None, local_knowledge: None });

		k.local_knowledge = Some(local_knowledge);
	}

	fn manifest_received_from(
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

	fn direct_statement_senders(
		&self,
		group_index: GroupIndex,
		originator_index_in_group: usize,
		statement_kind: StatementKind,
	) -> Vec<ValidatorIndex> {
		if group_index != self.group_index {
			return Vec::new()
		}

		self.mutual_knowledge
			.iter()
			.filter(|(_, k)| k.remote_knowledge.is_some())
			.filter(|(_, k)| {
				k.local_knowledge
					.as_ref()
					.map_or(false, |r| !r.contains(originator_index_in_group, statement_kind))
			})
			.map(|(v, _)| *v)
			.collect()
	}

	fn direct_statement_recipients(
		&self,
		group_index: GroupIndex,
		originator_index_in_group: usize,
		statement_kind: StatementKind,
	) -> Vec<ValidatorIndex> {
		if group_index != self.group_index {
			return Vec::new()
		}

		self.mutual_knowledge
			.iter()
			.filter(|(_, k)| k.local_knowledge.is_some())
			.filter(|(_, k)| {
				k.remote_knowledge
					.as_ref()
					.map_or(false, |r| !r.contains(originator_index_in_group, statement_kind))
			})
			.map(|(v, _)| *v)
			.collect()
	}

	fn note_fresh_statement(
		&mut self,
		statement_index_in_group: usize,
		statement_kind: StatementKind,
	) -> bool {
		let really_fresh = !self.local_knowledge.contains(statement_index_in_group, statement_kind);
		self.local_knowledge.set(statement_index_in_group, statement_kind);

		really_fresh
	}

	fn sent_or_received_direct_statement(
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

	fn is_pending_statement(
		&self,
		validator: ValidatorIndex,
		statement_index_in_group: usize,
		statement_kind: StatementKind,
	) -> bool {
		// existence of both remote & local knowledge indicate we have exchanged
		// manifests.
		// then, everything that is not in the remote knowledge is pending
		self.mutual_knowledge
			.get(&validator)
			.filter(|k| k.local_knowledge.is_some())
			.and_then(|k| k.remote_knowledge.as_ref())
			.map(|k| !k.contains(statement_index_in_group, statement_kind))
			.unwrap_or(false)
	}

	fn pending_statements(&self, validator: ValidatorIndex) -> Option<StatementFilter> {
		// existence of both remote & local knowledge indicate we have exchanged
		// manifests.
		// then, everything that is not in the remote knowledge is pending, and we
		// further limit this by what is in the local knowledge itself. we use the
		// full local knowledge, as the local knowledge stored here may be outdated.
		let full_local = &self.local_knowledge;

		self.mutual_knowledge
			.get(&validator)
			.filter(|k| k.local_knowledge.is_some())
			.and_then(|k| k.remote_knowledge.as_ref())
			.map(|remote| StatementFilter {
				seconded_in_group: full_local.seconded_in_group.clone() &
					!remote.seconded_in_group.clone(),
				validated_in_group: full_local.validated_in_group.clone() &
					!remote.validated_in_group.clone(),
			})
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
		let mut tracker = GridTracker::default();
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
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
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
				ManifestKind::Full,
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
				ManifestKind::Full,
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Disallowed)
		);
	}

	#[test]
	fn reject_malformed_wrong_group_size() {
		let mut tracker = GridTracker::default();
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
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
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
				ManifestKind::Full,
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
				ManifestKind::Full,
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);
	}

	#[test]
	fn reject_malformed_no_seconders() {
		let mut tracker = GridTracker::default();
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
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
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
				ManifestKind::Full,
				ValidatorIndex(0),
			),
			Err(ManifestImportError::Malformed)
		);
	}

	#[test]
	fn reject_malformed_below_threshold() {
		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: HashSet::from([ValidatorIndex(0)]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
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
				ManifestKind::Full,
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
				ManifestKind::Full,
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
				ManifestKind::Full,
				ValidatorIndex(0),
			),
			Ok(false)
		);
	}

	#[test]
	fn senders_can_provide_manifests_in_acknowledgement() {
		let validator_index = ValidatorIndex(0);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::from([validator_index]),
					receiving: HashSet::from([ValidatorIndex(1)]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		assert_eq!(groups.get_size_and_backing_threshold(group_index), Some((group_size, 2)));

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Note the manifest as 'sent' to validator 0.
		tracker.manifest_sent_to(&groups, validator_index, candidate_hash, local_knowledge);

		// Import manifest.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Acknowledgement,
			validator_index,
		);
		assert_matches!(ack, Ok(false));
	}

	// Check that pending communication is set correctly when receiving a manifest on a confirmed candidate.
	//
	// It should also overwrite any existing `Full` ManifestKind.
	#[test]
	fn pending_communication_receiving_manifest_on_confirmed_candidate() {
		let validator_index = ValidatorIndex(0);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::from([validator_index]),
					receiving: HashSet::from([ValidatorIndex(1)]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		assert_eq!(groups.get_size_and_backing_threshold(group_index), Some((group_size, 2)));

		// Manifest should not be pending yet.
		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, None);

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Manifest should be pending as `Full`.
		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, Some(ManifestKind::Full));

		// Note the manifest as 'sent' to validator 0.
		tracker.manifest_sent_to(&groups, validator_index, candidate_hash, local_knowledge);

		// Import manifest.
		//
		// Should overwrite existing `Full` manifest.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Acknowledgement,
			validator_index,
		);
		assert_matches!(ack, Ok(false));

		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, None);
	}

	// Check that pending communication is cleared correctly in `manifest_sent_to`
	//
	// Also test a scenario where manifest import returns `Ok(true)`.
	#[test]
	fn pending_communication_is_cleared() {
		let validator_index = ValidatorIndex(0);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: HashSet::from([validator_index]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		assert_eq!(groups.get_size_and_backing_threshold(group_index), Some((group_size, 2)));

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Manifest should not be pending yet.
		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, None);

		// Import manifest.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Full,
			validator_index,
		);
		assert_matches!(ack, Ok(true));

		// Acknowledgement manifest should be pending.
		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, Some(ManifestKind::Acknowledgement));

		// Note the candidate as advertised.
		tracker.manifest_sent_to(&groups, validator_index, candidate_hash, local_knowledge);

		// Pending manifest should be cleared.
		let pending_manifest = tracker.is_manifest_pending_for(validator_index, &candidate_hash);
		assert_eq!(pending_manifest, None);
	}

	#[test]
	fn pending_statements_are_updated_after_manifest_exchange() {
		let mut tracker = GridTracker::default();

		let validator_index = ValidatorIndex(0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_size = 3;

		// Should start with no pending statements.
		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);

		let statement_filter = StatementFilter::new(group_size);

		todo!()
	}

	#[test]
	fn invalid_fresh_statement_import() {
		let validator_index = ValidatorIndex(0);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: HashSet::from([validator_index]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		// Should start with no pending statements.
		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);

		// Import fresh statement. Candidate not backed.
		let statement = CompactStatement::Seconded(candidate_hash);
		tracker.learned_fresh_statement(&groups, &session_topology, ValidatorIndex(1), &statement);

		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Import fresh statement. Unknown group for validator index.
		let statement = CompactStatement::Seconded(candidate_hash);
		tracker.learned_fresh_statement(&groups, &session_topology, ValidatorIndex(1), &statement);

		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);
	}

	#[test]
	fn pending_statements_updated_when_importing_fresh_statement() {
		let validator_index = ValidatorIndex(0);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: HashSet::from([validator_index]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		// Should start with no pending statements.
		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Import fresh statement.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Full,
			validator_index,
		);
		tracker.manifest_sent_to(&groups, validator_index, candidate_hash, local_knowledge);
		let statement = CompactStatement::Seconded(candidate_hash);
		tracker.learned_fresh_statement(&groups, &session_topology, validator_index, &statement);

		// There should be pending statements now.
		let statements = StatementFilter {
			seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 0],
			validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
		};
		assert_eq!(
			tracker.pending_statements_for(validator_index, candidate_hash),
			Some(statements.clone())
		);
		assert_eq!(
			tracker.all_pending_statements_for(validator_index),
			vec![(ValidatorIndex(0), CompactStatement::Seconded(candidate_hash))]
		);

		// After successful import, try importing again. Nothing should change.
		tracker.learned_fresh_statement(&groups, &session_topology, validator_index, &statement);
		assert_eq!(
			tracker.pending_statements_for(validator_index, candidate_hash),
			Some(statements)
		);
		assert_eq!(
			tracker.all_pending_statements_for(validator_index),
			vec![(ValidatorIndex(0), CompactStatement::Seconded(candidate_hash))]
		);
	}

	// TODO [now]: test that pending statements respect remote knowledge

	#[test]
	fn pending_statements_cleared_when_sending() {
		let validator_index = ValidatorIndex(0);
		let counterparty = ValidatorIndex(1);

		let mut tracker = GridTracker::default();
		let session_topology = SessionTopologyView {
			group_views: vec![(
				GroupIndex(0),
				GroupSubView {
					sending: HashSet::new(),
					receiving: HashSet::from([validator_index, counterparty]),
				},
			)]
			.into_iter()
			.collect(),
		};

		let groups = Groups::new(
			vec![vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)]].into(),
			&[
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
				AuthorityDiscoveryPair::generate().0.public(),
			],
		);

		let candidate_hash = CandidateHash(Hash::repeat_byte(42));
		let group_index = GroupIndex(0);
		let group_size = 3;
		let local_knowledge = StatementFilter::new(group_size);

		// Should start with no pending statements.
		assert_eq!(tracker.pending_statements_for(validator_index, candidate_hash), None);
		assert_eq!(tracker.all_pending_statements_for(validator_index), vec![]);

		// Add the candidate as backed.
		tracker.add_backed_candidate(
			&session_topology,
			candidate_hash,
			group_index,
			group_size,
			local_knowledge.clone(),
		);

		// Import statement for originator.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Full,
			validator_index,
		);
		tracker.manifest_sent_to(&groups, validator_index, candidate_hash, local_knowledge.clone());
		let statement = CompactStatement::Seconded(candidate_hash);
		tracker.learned_fresh_statement(&groups, &session_topology, validator_index, &statement);

		// Import statement for counterparty.
		let ack = tracker.import_manifest(
			&session_topology,
			&groups,
			candidate_hash,
			3,
			ManifestSummary {
				claimed_parent_hash: Hash::repeat_byte(0),
				claimed_group_index: group_index,
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			},
			ManifestKind::Full,
			counterparty,
		);
		tracker.manifest_sent_to(&groups, counterparty, candidate_hash, local_knowledge);
		let statement = CompactStatement::Seconded(candidate_hash);
		tracker.learned_fresh_statement(&groups, &session_topology, counterparty, &statement);

		// There should be pending statements now.
		let statements = StatementFilter {
			seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 0],
			validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
		};
		assert_eq!(
			tracker.pending_statements_for(validator_index, candidate_hash),
			Some(statements.clone())
		);
		assert_eq!(
			tracker.all_pending_statements_for(validator_index),
			vec![(ValidatorIndex(0), CompactStatement::Seconded(candidate_hash))]
		);
		assert_eq!(
			tracker.pending_statements_for(counterparty, candidate_hash),
			Some(statements.clone())
		);
		assert_eq!(
			tracker.all_pending_statements_for(counterparty),
			vec![(ValidatorIndex(0), CompactStatement::Seconded(candidate_hash))]
		);

		// Note sending or receiving of a direct statement.
		tracker.sent_or_received_direct_statement(
			&groups,
			validator_index,
			counterparty,
			&statement,
		);

		// There should be no pending statements now (for the counterparty).
		assert_eq!(tracker.all_pending_statements_for(counterparty), vec![]);
		// TODO: There are still `pending_statements_for`, is this correct?
		// assert_eq!(tracker.pending_statements_for(counterparty, candidate_hash), None);
	}
}
