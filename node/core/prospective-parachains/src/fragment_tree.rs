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

//! A tree utility for managing parachain fragments not referenced by the relay-chain.
//!
//! This module exposes two main types: [`FragmentTree`] and [`CandidateStorage`]
//! which are meant to be used in close conjunction. Each tree is associated with a particular
//! relay-parent, and it's expected that higher-level code will have a tree for each
//! relay-chain block which might reasonably have blocks built upon it.
//!
//! Trees only store indices into the [`CandidateStorage`] and the storage is meant to
//! be pruned when trees are dropped by higher-level code.
//!
//! Each node in the tree represents a candidate. Nodes do not uniquely refer to a parachain
//! block for two reasons.
//!   1. There's no requirement that head-data is unique
//!      for a parachain. Furthermore, a parachain is under no obligation to be acyclic, and this is mostly
//!      just because it's totally inefficient to enforce it. Practical use-cases are acyclic, but there is
//!      still more than one way to reach the same head-data.
//!   2. and candidates only refer to their parent by its head-data. This whole issue could be
//!      resolved by having candidates reference their parent by candidate hash.
//!
//! The implication is that when we receive a candidate receipt, there are actually multiple
//! possibilities for any candidates between the para-head recorded in the relay parent's state
//! and the candidate in question.
//!
//! This means that our candidates need to handle multiple parents and that depth is an
//! attribute of a node in a tree, not a candidate. Put another way, the same candidate might
//! have different depths in different parts of the tree.
//!
//! As an extreme example, a candidate which produces head-data which is the same as its parent
//! can correspond to multiple nodes within the same [`FragmentTree`]. Such cycles are bounded
//! by the maximum depth allowed by the tree.
//!
//! As long as the [`CandidateStorage`] has bounded input on the number of candidates supplied,
//! [`FragmentTree`] complexity is bounded. This means that higher-level code needs to be selective
//! about limiting the amount of candidates that are considered.
//!
//! The code in this module is not designed for speed or efficiency, but conceptual simplicity.
//! Our assumption is that the amount of candidates and parachains we consider will be reasonably
//! bounded and in practice will not exceed a few thousand at any time. This naive implementation
//! will still perform fairly well under these conditions, despite being somewhat wasteful of memory.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::LOG_TARGET;
use bitvec::prelude::*;
use polkadot_node_subsystem_util::inclusion_emulator::staging::{
	ConstraintModifications, Constraints, Fragment, ProspectiveCandidate, RelayChainBlockInfo,
};
use polkadot_primitives::vstaging::{
	BlockNumber, CandidateHash, CommittedCandidateReceipt, Hash, HeadData, Id as ParaId,
	PersistedValidationData,
};

/// Kinds of failures to import a candidate into storage.
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateStorageInsertionError {
	/// An error indicating that a supplied candidate didn't match the persisted
	/// validation data provided alongside it.
	PersistedValidationDataMismatch,
	/// The candidate was already known.
	CandidateAlreadyKnown(CandidateHash),
}

pub(crate) struct CandidateStorage {
	// Index from parent head hash to candidate hashes.
	by_parent_head: HashMap<Hash, HashSet<CandidateHash>>,

	// Index from candidate hash to fragment node.
	by_candidate_hash: HashMap<CandidateHash, CandidateEntry>,
}

impl CandidateStorage {
	/// Create a new `CandidateStorage`.
	pub fn new() -> Self {
		CandidateStorage { by_parent_head: HashMap::new(), by_candidate_hash: HashMap::new() }
	}

	/// Introduce a new candidate. The candidate passed to this function
	/// should have been seconded before introduction.
	pub fn add_candidate(
		&mut self,
		candidate: CommittedCandidateReceipt,
		persisted_validation_data: PersistedValidationData,
	) -> Result<CandidateHash, CandidateStorageInsertionError> {
		let candidate_hash = candidate.hash();

		if self.by_candidate_hash.contains_key(&candidate_hash) {
			return Err(CandidateStorageInsertionError::CandidateAlreadyKnown(candidate_hash))
		}

		if persisted_validation_data.hash() != candidate.descriptor.persisted_validation_data_hash {
			return Err(CandidateStorageInsertionError::PersistedValidationDataMismatch)
		}

		let parent_head_hash = persisted_validation_data.parent_head.hash();

		let entry = CandidateEntry {
			candidate_hash,
			relay_parent: candidate.descriptor.relay_parent,
			state: CandidateState::Seconded,
			candidate: ProspectiveCandidate {
				commitments: candidate.commitments,
				collator: candidate.descriptor.collator,
				collator_signature: candidate.descriptor.signature,
				persisted_validation_data,
				pov_hash: candidate.descriptor.pov_hash,
				validation_code_hash: candidate.descriptor.validation_code_hash,
			},
		};

		self.by_parent_head.entry(parent_head_hash).or_default().insert(candidate_hash);
		// sanity-checked already.
		self.by_candidate_hash.insert(candidate_hash, entry);

		Ok(candidate_hash)
	}

	/// Note that an existing candidate has been backed.
	pub fn mark_backed(&mut self, candidate_hash: &CandidateHash) {
		if let Some(entry) = self.by_candidate_hash.get_mut(candidate_hash) {
			entry.state = CandidateState::Backed;
		}
	}

	/// Whether a candidate is recorded as being backed.
	pub fn is_backed(&self, candidate_hash: &CandidateHash) -> bool {
		self.by_candidate_hash
			.get(candidate_hash)
			.map_or(false, |e| e.state == CandidateState::Backed)
	}

	/// Whether a candidate is contained within the storage already.
	pub fn contains(&self, candidate_hash: &CandidateHash) -> bool {
		self.by_candidate_hash.contains_key(candidate_hash)
	}

	/// Retain only candidates which pass the predicate.
	pub(crate) fn retain(&mut self, pred: impl Fn(&CandidateHash) -> bool) {
		self.by_candidate_hash.retain(|h, _v| pred(h));
		self.by_parent_head.retain(|_parent, children| {
			children.retain(|h| pred(h));
			!children.is_empty()
		})
	}

	/// Get head-data by hash.
	pub(crate) fn head_data_by_hash(&self, hash: &Hash) -> Option<&HeadData> {
		// Get some candidate which has a parent-head with the same hash as requested.
		let a_candidate_hash = self.by_parent_head.get(hash).and_then(|m| m.iter().next())?;

		// Extract the full parent head from that candidate's `PersistedValidationData`.
		self.by_candidate_hash
			.get(a_candidate_hash)
			.map(|e| &e.candidate.persisted_validation_data.parent_head)
	}

	fn iter_para_children<'a>(
		&'a self,
		parent_head_hash: &Hash,
	) -> impl Iterator<Item = &'a CandidateEntry> + 'a {
		let by_candidate_hash = &self.by_candidate_hash;
		self.by_parent_head
			.get(parent_head_hash)
			.into_iter()
			.flat_map(|hashes| hashes.iter())
			.filter_map(move |h| by_candidate_hash.get(h))
	}

	fn get(&'_ self, candidate_hash: &CandidateHash) -> Option<&'_ CandidateEntry> {
		self.by_candidate_hash.get(candidate_hash)
	}
}

/// The state of a candidate.
///
/// Candidates aren't even considered until they've at least been seconded.
#[derive(Debug, PartialEq)]
enum CandidateState {
	/// The candidate has been seconded.
	Seconded,
	/// The candidate has been completely backed by the group.
	Backed,
}

struct CandidateEntry {
	candidate_hash: CandidateHash,
	relay_parent: Hash,
	candidate: ProspectiveCandidate,
	state: CandidateState,
}

/// The scope of a [`FragmentTree`].
#[derive(Debug)]
pub(crate) struct Scope {
	para: ParaId,
	relay_parent: RelayChainBlockInfo,
	ancestors: BTreeMap<BlockNumber, RelayChainBlockInfo>,
	ancestors_by_hash: HashMap<Hash, RelayChainBlockInfo>,
	base_constraints: Constraints,
	max_depth: usize,
}

/// An error variant indicating that ancestors provided to a scope
/// had unexpected order.
#[derive(Debug)]
pub struct UnexpectedAncestor;

impl Scope {
	/// Define a new [`Scope`].
	///
	/// All arguments are straightforward except the ancestors.
	///
	/// Ancestors should be in reverse order, starting with the parent
	/// of the `relay_parent`, and proceeding backwards in block number
	/// increments of 1. Ancestors not following these conditions will be
	/// rejected.
	///
	/// This function will only consume ancestors up to the `min_relay_parent_number` of
	/// the `base_constraints`.
	///
	/// Only ancestors whose children have the same session as the relay-parent's
	/// children should be provided.
	///
	/// It is allowed to provide zero ancestors.
	pub fn with_ancestors(
		para: ParaId,
		relay_parent: RelayChainBlockInfo,
		base_constraints: Constraints,
		max_depth: usize,
		ancestors: impl IntoIterator<Item = RelayChainBlockInfo>,
	) -> Result<Self, UnexpectedAncestor> {
		let mut ancestors_map = BTreeMap::new();
		let mut ancestors_by_hash = HashMap::new();
		{
			let mut prev = relay_parent.number;
			for ancestor in ancestors {
				if prev == 0 {
					return Err(UnexpectedAncestor)
				} else if ancestor.number != prev - 1 {
					return Err(UnexpectedAncestor)
				} else if prev == base_constraints.min_relay_parent_number {
					break
				} else {
					prev = ancestor.number;
					ancestors_by_hash.insert(ancestor.hash, ancestor.clone());
					ancestors_map.insert(ancestor.number, ancestor);
				}
			}
		}

		Ok(Scope {
			para,
			relay_parent,
			base_constraints,
			max_depth,
			ancestors: ancestors_map,
			ancestors_by_hash,
		})
	}

	/// Get the earliest relay-parent allowed in the scope of the fragment tree.
	pub fn earliest_relay_parent(&self) -> RelayChainBlockInfo {
		self.ancestors
			.iter()
			.next()
			.map(|(_, v)| v.clone())
			.unwrap_or_else(|| self.relay_parent.clone())
	}

	/// Get the ancestor of the fragment tree by hash.
	pub fn ancestor_by_hash(&self, hash: &Hash) -> Option<RelayChainBlockInfo> {
		if hash == &self.relay_parent.hash {
			return Some(self.relay_parent.clone())
		}

		self.ancestors_by_hash.get(hash).map(|info| info.clone())
	}

	/// Get the base constraints of the scope
	pub fn base_constraints(&self) -> &Constraints {
		&self.base_constraints
	}
}

// We use indices into a flat vector to refer to nodes in the tree.
// Every tree also has an implicit root.
#[derive(Debug, Clone, Copy, PartialEq)]
enum NodePointer {
	Root,
	Storage(usize),
}

/// This is a tree of candidates based on some underlying storage of candidates
/// and a scope.
pub(crate) struct FragmentTree {
	scope: Scope,

	// Invariant: a contiguous prefix of the 'nodes' storage will contain
	// the top-level children.
	nodes: Vec<FragmentNode>,

	// The candidates stored in this tree, mapped to a bitvec indicating the depths
	// where the candidate is stored.
	candidates: HashMap<CandidateHash, BitVec<u16, Msb0>>,
}

impl FragmentTree {
	/// Create a new [`FragmentTree`] with given scope and populated from the
	/// storage.
	pub fn populate(scope: Scope, storage: &CandidateStorage) -> Self {
		gum::trace!(
			target: LOG_TARGET,
			relay_parent = ?scope.relay_parent.hash,
			relay_parent_num = scope.relay_parent.number,
			para_id = ?scope.para,
			ancestors = scope.ancestors.len(),
			"Instantiating Fragment Tree",
		);

		let mut tree = FragmentTree { scope, nodes: Vec::new(), candidates: HashMap::new() };

		tree.populate_from_bases(storage, vec![NodePointer::Root]);

		tree
	}

	/// Get the scope of the Fragment Tree.
	pub fn scope(&self) -> &Scope {
		&self.scope
	}

	// Inserts a node and updates child references in a non-root parent.
	fn insert_node(&mut self, node: FragmentNode) {
		let pointer = NodePointer::Storage(self.nodes.len());
		let parent_pointer = node.parent;
		let candidate_hash = node.candidate_hash;

		let max_depth = self.scope.max_depth;

		self.candidates
			.entry(candidate_hash)
			.or_insert_with(|| bitvec![u16, Msb0; 0; max_depth + 1])
			.set(node.depth, true);

		match parent_pointer {
			NodePointer::Storage(ptr) => {
				self.nodes.push(node);
				self.nodes[ptr].children.push((pointer, candidate_hash))
			},
			NodePointer::Root => {
				// Maintain the invariant of node storage beginning with depth-0.
				if self.nodes.last().map_or(true, |last| last.parent == NodePointer::Root) {
					self.nodes.push(node);
				} else {
					let pos =
						self.nodes.iter().take_while(|n| n.parent == NodePointer::Root).count();
					self.nodes.insert(pos, node);
				}
			},
		}
	}

	fn node_has_candidate_child(
		&self,
		pointer: NodePointer,
		candidate_hash: &CandidateHash,
	) -> bool {
		self.node_candidate_child(pointer, candidate_hash).is_some()
	}

	fn node_candidate_child(
		&self,
		pointer: NodePointer,
		candidate_hash: &CandidateHash,
	) -> Option<NodePointer> {
		match pointer {
			NodePointer::Root => self
				.nodes
				.iter()
				.take_while(|n| n.parent == NodePointer::Root)
				.enumerate()
				.find(|(_, n)| &n.candidate_hash == candidate_hash)
				.map(|(i, _)| NodePointer::Storage(i)),
			NodePointer::Storage(ptr) =>
				self.nodes.get(ptr).and_then(|n| n.candidate_child(candidate_hash)),
		}
	}

	/// Returns an O(n) iterator over the hashes of candidates contained in the
	/// tree.
	pub(crate) fn candidates<'a>(&'a self) -> impl Iterator<Item = CandidateHash> + 'a {
		self.candidates.keys().cloned()
	}

	/// Whether the candidate exists and at what depths.
	pub(crate) fn candidate(&self, candidate: &CandidateHash) -> Option<Vec<usize>> {
		self.candidates.get(candidate).map(|d| d.iter_ones().collect())
	}

	/// Add a candidate and recursively populate from storage.
	pub(crate) fn add_and_populate(&mut self, hash: CandidateHash, storage: &CandidateStorage) {
		let candidate_entry = match storage.get(&hash) {
			None => return,
			Some(e) => e,
		};

		let candidate_parent = &candidate_entry.candidate.persisted_validation_data.parent_head;

		// Select an initial set of bases, whose required relay-parent matches that of the candidate.
		let root_base = if &self.scope.base_constraints.required_parent == candidate_parent {
			Some(NodePointer::Root)
		} else {
			None
		};

		let non_root_bases = self
			.nodes
			.iter()
			.enumerate()
			.filter(|(_, n)| {
				n.cumulative_modifications.required_parent.as_ref() == Some(candidate_parent)
			})
			.map(|(i, _)| NodePointer::Storage(i));

		let bases = root_base.into_iter().chain(non_root_bases).collect();

		// Pass this into the population function, which will sanity-check stuff like depth, fragments,
		// etc. and then recursively populate.
		self.populate_from_bases(storage, bases);
	}

	/// Returns the hypothetical depths where a candidate with the given hash and parent head data
	/// would be added to the tree, without applying other candidates recursively on top of it.
	///
	/// If the candidate is already known, this returns the actual depths where this
	/// candidate is part of the tree.
	pub(crate) fn hypothetical_depths(
		&self,
		hash: CandidateHash,
		parent_head_data_hash: Hash,
		candidate_relay_parent: Hash,
	) -> Vec<usize> {
		// if known.
		if let Some(depths) = self.candidates.get(&hash) {
			return depths.iter_ones().collect()
		}

		// if out of scope.
		let candidate_relay_parent_number =
			if self.scope.relay_parent.hash == candidate_relay_parent {
				self.scope.relay_parent.number
			} else if let Some(info) = self.scope.ancestors_by_hash.get(&candidate_relay_parent) {
				info.number
			} else {
				return Vec::new()
			};

		let max_depth = self.scope.max_depth;
		let mut depths = bitvec![u16, Msb0; 0; max_depth + 1];

		// iterate over all nodes < max_depth where parent head-data matches,
		// relay-parent number is <= candidate, and depth < max_depth.
		for node in &self.nodes {
			if node.depth == max_depth {
				continue
			}
			if node.fragment.relay_parent().number > candidate_relay_parent_number {
				continue
			}
			if node.head_data_hash == parent_head_data_hash {
				depths.set(node.depth + 1, true);
			}
		}

		// compare against root as well.
		if self.scope.base_constraints.required_parent.hash() == parent_head_data_hash {
			depths.set(0, true);
		}

		depths.iter_ones().collect()
	}

	/// Select a candidate after the given `required_path` which pass
	/// the predicate.
	///
	/// If there are multiple possibilities, this will select the first one.
	///
	/// This returns `None` if there is no candidate meeting those criteria.
	///
	/// The intention of the `required_path` is to allow queries on the basis of
	/// one or more candidates which were previously pending availability becoming
	/// available and opening up more room on the core.
	pub(crate) fn select_child(
		&self,
		required_path: &[CandidateHash],
		pred: impl Fn(&CandidateHash) -> bool,
	) -> Option<CandidateHash> {
		let base_node = {
			// traverse the required path.
			let mut node = NodePointer::Root;
			for required_step in required_path {
				node = self.node_candidate_child(node, &required_step)?;
			}

			node
		};

		// TODO [now]: taking the first selection might introduce bias
		// or become gameable.
		//
		// For plausibly unique parachains, this shouldn't matter much.
		// figure out alternative selection criteria?
		match base_node {
			NodePointer::Root => self
				.nodes
				.iter()
				.take_while(|n| n.parent == NodePointer::Root)
				.filter(|n| pred(&n.candidate_hash))
				.map(|n| n.candidate_hash)
				.next(),
			NodePointer::Storage(ptr) =>
				self.nodes[ptr].children.iter().filter(|n| pred(&n.1)).map(|n| n.1).next(),
		}
	}

	fn populate_from_bases<'a>(
		&mut self,
		storage: &'a CandidateStorage,
		initial_bases: Vec<NodePointer>,
	) {
		// Populate the tree breadth-first.
		let mut last_sweep_start = None;

		loop {
			let sweep_start = self.nodes.len();

			if Some(sweep_start) == last_sweep_start {
				break
			}

			let parents: Vec<NodePointer> = if let Some(last_start) = last_sweep_start {
				(last_start..self.nodes.len()).map(NodePointer::Storage).collect()
			} else {
				initial_bases.clone()
			};

			// 1. get parent head and find constraints
			// 2. iterate all candidates building on the right head and viable relay parent
			// 3. add new node
			for parent_pointer in parents {
				let (modifications, child_depth, earliest_rp) = match parent_pointer {
					NodePointer::Root =>
						(ConstraintModifications::identity(), 0, self.scope.earliest_relay_parent()),
					NodePointer::Storage(ptr) => {
						let node = &self.nodes[ptr];
						let parent_rp = self
							.scope
							.ancestor_by_hash(&node.relay_parent())
							.expect("nodes in tree can only contain ancestors within scope; qed");

						(node.cumulative_modifications.clone(), node.depth + 1, parent_rp)
					},
				};

				if child_depth > self.scope.max_depth {
					continue
				}

				let child_constraints =
					match self.scope.base_constraints.apply_modifications(&modifications) {
						Err(e) => {
							gum::debug!(
								target: LOG_TARGET,
								new_parent_head = ?modifications.required_parent,
								err = ?e,
								"Failed to apply modifications",
							);

							continue
						},
						Ok(c) => c,
					};

				// Add nodes to tree wherever
				// 1. parent hash is correct
				// 2. relay-parent does not move backwards
				// 3. candidate outputs fulfill constraints
				let required_head_hash = child_constraints.required_parent.hash();
				for candidate in storage.iter_para_children(&required_head_hash) {
					let relay_parent = match self.scope.ancestor_by_hash(&candidate.relay_parent) {
						None => continue, // not in chain
						Some(info) => {
							if info.number < earliest_rp.number {
								// moved backwards
								continue
							}

							info
						},
					};

					// don't add candidates where the parent already has it as a child.
					if self.node_has_candidate_child(parent_pointer, &candidate.candidate_hash) {
						continue
					}

					let fragment = {
						let f = Fragment::new(
							relay_parent.clone(),
							child_constraints.clone(),
							candidate.candidate.clone(),
						);

						match f {
							Ok(f) => f,
							Err(e) => {
								gum::debug!(
									target: LOG_TARGET,
									err = ?e,
									?relay_parent,
									candidate_hash = ?candidate.candidate_hash,
									"Failed to instantiate fragment",
								);

								continue
							},
						}
					};

					let mut cumulative_modifications = modifications.clone();
					cumulative_modifications.stack(fragment.constraint_modifications());

					let head_data_hash = fragment.candidate().commitments.head_data.hash();
					let node = FragmentNode {
						parent: parent_pointer,
						fragment,
						candidate_hash: candidate.candidate_hash.clone(),
						depth: child_depth,
						cumulative_modifications,
						children: Vec::new(),
						head_data_hash,
					};

					self.insert_node(node);
				}
			}

			last_sweep_start = Some(sweep_start);
		}
	}
}

struct FragmentNode {
	// A pointer to the parent node.
	parent: NodePointer,
	fragment: Fragment,
	candidate_hash: CandidateHash,
	depth: usize,
	cumulative_modifications: ConstraintModifications,
	head_data_hash: Hash,
	children: Vec<(NodePointer, CandidateHash)>,
}

impl FragmentNode {
	fn relay_parent(&self) -> Hash {
		self.fragment.relay_parent().hash
	}

	fn candidate_child(&self, candidate_hash: &CandidateHash) -> Option<NodePointer> {
		self.children.iter().find(|(_, c)| c == candidate_hash).map(|(p, _)| *p)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use polkadot_node_subsystem_util::inclusion_emulator::staging::InboundHrmpLimitations;
	use polkadot_primitives::vstaging::{
		BlockNumber, CandidateCommitments, CandidateDescriptor, HeadData,
	};
	use polkadot_primitives_test_helpers as test_helpers;

	fn make_constraints(
		min_relay_parent_number: BlockNumber,
		valid_watermarks: Vec<BlockNumber>,
		required_parent: HeadData,
	) -> Constraints {
		Constraints {
			min_relay_parent_number,
			max_pov_size: 1_000_000,
			max_code_size: 1_000_000,
			ump_remaining: 10,
			ump_remaining_bytes: 1_000,
			max_ump_num_per_candidate: 10,
			dmp_remaining_messages: 10,
			hrmp_inbound: InboundHrmpLimitations { valid_watermarks },
			hrmp_channels_out: HashMap::new(),
			max_hrmp_num_per_candidate: 0,
			required_parent,
			validation_code_hash: Hash::repeat_byte(42).into(),
			upgrade_restriction: None,
			future_validation_code: None,
		}
	}

	fn make_committed_candidate(
		para_id: ParaId,
		relay_parent: Hash,
		relay_parent_number: BlockNumber,
		parent_head: HeadData,
		para_head: HeadData,
		hrmp_watermark: BlockNumber,
	) -> (PersistedValidationData, CommittedCandidateReceipt) {
		let persisted_validation_data = PersistedValidationData {
			parent_head,
			relay_parent_number,
			relay_parent_storage_root: Hash::repeat_byte(69),
			max_pov_size: 1_000_000,
		};

		let candidate = CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id,
				relay_parent,
				collator: test_helpers::dummy_collator(),
				persisted_validation_data_hash: persisted_validation_data.hash(),
				pov_hash: Hash::repeat_byte(1),
				erasure_root: Hash::repeat_byte(1),
				signature: test_helpers::dummy_collator_signature(),
				para_head: para_head.hash(),
				validation_code_hash: Hash::repeat_byte(42).into(),
			},
			commitments: CandidateCommitments {
				upward_messages: Vec::new(),
				horizontal_messages: Vec::new(),
				new_validation_code: None,
				head_data: para_head,
				processed_downward_messages: 0,
				hrmp_watermark,
			},
		};

		(persisted_validation_data, candidate)
	}

	#[test]
	fn scope_rejects_ancestors_that_skip_blocks() {
		let para_id = ParaId::from(5u32);
		let relay_parent = RelayChainBlockInfo {
			number: 10,
			hash: Hash::repeat_byte(10),
			storage_root: Hash::repeat_byte(69),
		};

		let ancestors = vec![RelayChainBlockInfo {
			number: 8,
			hash: Hash::repeat_byte(8),
			storage_root: Hash::repeat_byte(69),
		}];

		let max_depth = 2;
		let base_constraints = make_constraints(8, vec![8, 9], vec![1, 2, 3].into());

		assert_matches!(
			Scope::with_ancestors(para_id, relay_parent, base_constraints, max_depth, ancestors,),
			Err(UnexpectedAncestor)
		);
	}

	#[test]
	fn scope_rejects_ancestor_for_0_block() {
		let para_id = ParaId::from(5u32);
		let relay_parent = RelayChainBlockInfo {
			number: 0,
			hash: Hash::repeat_byte(0),
			storage_root: Hash::repeat_byte(69),
		};

		let ancestors = vec![RelayChainBlockInfo {
			number: 99999,
			hash: Hash::repeat_byte(99),
			storage_root: Hash::repeat_byte(69),
		}];

		let max_depth = 2;
		let base_constraints = make_constraints(0, vec![], vec![1, 2, 3].into());

		assert_matches!(
			Scope::with_ancestors(para_id, relay_parent, base_constraints, max_depth, ancestors,),
			Err(UnexpectedAncestor)
		);
	}

	#[test]
	fn scope_only_takes_ancestors_up_to_min() {
		let para_id = ParaId::from(5u32);
		let relay_parent = RelayChainBlockInfo {
			number: 5,
			hash: Hash::repeat_byte(0),
			storage_root: Hash::repeat_byte(69),
		};

		let ancestors = vec![
			RelayChainBlockInfo {
				number: 4,
				hash: Hash::repeat_byte(4),
				storage_root: Hash::repeat_byte(69),
			},
			RelayChainBlockInfo {
				number: 3,
				hash: Hash::repeat_byte(3),
				storage_root: Hash::repeat_byte(69),
			},
			RelayChainBlockInfo {
				number: 2,
				hash: Hash::repeat_byte(2),
				storage_root: Hash::repeat_byte(69),
			},
		];

		let max_depth = 2;
		let base_constraints = make_constraints(3, vec![2], vec![1, 2, 3].into());

		let scope =
			Scope::with_ancestors(para_id, relay_parent, base_constraints, max_depth, ancestors)
				.unwrap();

		assert_eq!(scope.ancestors.len(), 2);
		assert_eq!(scope.ancestors_by_hash.len(), 2);
	}

	#[test]
	fn storage_add_candidate() {
		let mut storage = CandidateStorage::new();

		let (pvd, candidate) = make_committed_candidate(
			ParaId::from(5u32),
			Hash::repeat_byte(69),
			8,
			vec![4, 5, 6].into(),
			vec![1, 2, 3].into(),
			7,
		);

		let candidate_hash = candidate.hash();
		let parent_head_hash = pvd.parent_head.hash();

		storage.add_candidate(candidate, pvd).unwrap();
		assert!(storage.contains(&candidate_hash));
		assert_eq!(storage.iter_para_children(&parent_head_hash).count(), 1);
	}

	#[test]
	fn storage_retain() {
		let mut storage = CandidateStorage::new();

		let (pvd, candidate) = make_committed_candidate(
			ParaId::from(5u32),
			Hash::repeat_byte(69),
			8,
			vec![4, 5, 6].into(),
			vec![1, 2, 3].into(),
			7,
		);

		let candidate_hash = candidate.hash();
		let parent_head_hash = pvd.parent_head.hash();

		storage.add_candidate(candidate, pvd).unwrap();
		storage.retain(|_| true);
		assert!(storage.contains(&candidate_hash));
		assert_eq!(storage.iter_para_children(&parent_head_hash).count(), 1);

		storage.retain(|_| false);
		assert!(!storage.contains(&candidate_hash));
		assert_eq!(storage.iter_para_children(&parent_head_hash).count(), 0);
	}

	#[test]
	fn populate_works_recursively() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);
		let relay_parent_b = Hash::repeat_byte(2);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(),
			0,
		);
		let candidate_a_hash = candidate_a.hash();

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_b,
			1,
			vec![0x0b].into(),
			vec![0x0c].into(),
			1,
		);
		let candidate_b_hash = candidate_b.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let ancestors = vec![RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		}];

		let relay_parent_b_info = RelayChainBlockInfo {
			number: pvd_b.relay_parent_number,
			hash: relay_parent_b,
			storage_root: pvd_b.relay_parent_storage_root,
		};

		storage.add_candidate(candidate_a, pvd_a).unwrap();
		storage.add_candidate(candidate_b, pvd_b).unwrap();
		let scope =
			Scope::with_ancestors(para_id, relay_parent_b_info, base_constraints, 4, ancestors)
				.unwrap();
		let tree = FragmentTree::populate(scope, &storage);

		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 2);
		assert!(candidates.contains(&candidate_a_hash));
		assert!(candidates.contains(&candidate_b_hash));

		assert_eq!(tree.nodes.len(), 2);
		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[0].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[0].depth, 0);

		assert_eq!(tree.nodes[1].parent, NodePointer::Storage(0));
		assert_eq!(tree.nodes[1].candidate_hash, candidate_b_hash);
		assert_eq!(tree.nodes[1].depth, 1);
	}

	#[test]
	fn children_of_root_are_contiguous() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);
		let relay_parent_b = Hash::repeat_byte(2);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(),
			0,
		);

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_b,
			1,
			vec![0x0b].into(),
			vec![0x0c].into(),
			1,
		);

		let (pvd_a2, candidate_a2) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b, 1].into(),
			0,
		);
		let candidate_a2_hash = candidate_a2.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let ancestors = vec![RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		}];

		let relay_parent_b_info = RelayChainBlockInfo {
			number: pvd_b.relay_parent_number,
			hash: relay_parent_b,
			storage_root: pvd_b.relay_parent_storage_root,
		};

		storage.add_candidate(candidate_a, pvd_a).unwrap();
		storage.add_candidate(candidate_b, pvd_b).unwrap();
		let scope =
			Scope::with_ancestors(para_id, relay_parent_b_info, base_constraints, 4, ancestors)
				.unwrap();
		let mut tree = FragmentTree::populate(scope, &storage);

		storage.add_candidate(candidate_a2, pvd_a2).unwrap();
		tree.add_and_populate(candidate_a2_hash, &storage);
		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 3);

		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[1].parent, NodePointer::Root);
		assert_eq!(tree.nodes[2].parent, NodePointer::Storage(0));
	}

	#[test]
	fn add_candidate_child_of_root() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(),
			0,
		);

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0c].into(),
			0,
		);
		let candidate_b_hash = candidate_b.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let relay_parent_a_info = RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		};

		storage.add_candidate(candidate_a, pvd_a).unwrap();
		let scope =
			Scope::with_ancestors(para_id, relay_parent_a_info, base_constraints, 4, vec![])
				.unwrap();
		let mut tree = FragmentTree::populate(scope, &storage);

		storage.add_candidate(candidate_b, pvd_b).unwrap();
		tree.add_and_populate(candidate_b_hash, &storage);
		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 2);

		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[1].parent, NodePointer::Root);
	}

	#[test]
	fn add_candidate_child_of_non_root() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(),
			0,
		);

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0b].into(),
			vec![0x0c].into(),
			0,
		);
		let candidate_b_hash = candidate_b.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let relay_parent_a_info = RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		};

		storage.add_candidate(candidate_a, pvd_a).unwrap();
		let scope =
			Scope::with_ancestors(para_id, relay_parent_a_info, base_constraints, 4, vec![])
				.unwrap();
		let mut tree = FragmentTree::populate(scope, &storage);

		storage.add_candidate(candidate_b, pvd_b).unwrap();
		tree.add_and_populate(candidate_b_hash, &storage);
		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 2);

		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[1].parent, NodePointer::Storage(0));
	}

	#[test]
	fn graceful_cycle_of_0() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0a].into(), // input same as output
			0,
		);
		let candidate_a_hash = candidate_a.hash();
		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let relay_parent_a_info = RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		};

		let max_depth = 4;
		storage.add_candidate(candidate_a, pvd_a).unwrap();
		let scope = Scope::with_ancestors(
			para_id,
			relay_parent_a_info,
			base_constraints,
			max_depth,
			vec![],
		)
		.unwrap();
		let tree = FragmentTree::populate(scope, &storage);

		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 1);
		assert_eq!(tree.nodes.len(), max_depth + 1);

		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[1].parent, NodePointer::Storage(0));
		assert_eq!(tree.nodes[2].parent, NodePointer::Storage(1));
		assert_eq!(tree.nodes[3].parent, NodePointer::Storage(2));
		assert_eq!(tree.nodes[4].parent, NodePointer::Storage(3));

		assert_eq!(tree.nodes[0].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[1].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[2].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[3].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[4].candidate_hash, candidate_a_hash);
	}

	#[test]
	fn graceful_cycle_of_1() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(), // input same as output
			0,
		);
		let candidate_a_hash = candidate_a.hash();

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0b].into(),
			vec![0x0a].into(), // input same as output
			0,
		);
		let candidate_b_hash = candidate_b.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let relay_parent_a_info = RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		};

		let max_depth = 4;
		storage.add_candidate(candidate_a, pvd_a).unwrap();
		storage.add_candidate(candidate_b, pvd_b).unwrap();
		let scope = Scope::with_ancestors(
			para_id,
			relay_parent_a_info,
			base_constraints,
			max_depth,
			vec![],
		)
		.unwrap();
		let tree = FragmentTree::populate(scope, &storage);

		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 2);
		assert_eq!(tree.nodes.len(), max_depth + 1);

		assert_eq!(tree.nodes[0].parent, NodePointer::Root);
		assert_eq!(tree.nodes[1].parent, NodePointer::Storage(0));
		assert_eq!(tree.nodes[2].parent, NodePointer::Storage(1));
		assert_eq!(tree.nodes[3].parent, NodePointer::Storage(2));
		assert_eq!(tree.nodes[4].parent, NodePointer::Storage(3));

		assert_eq!(tree.nodes[0].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[1].candidate_hash, candidate_b_hash);
		assert_eq!(tree.nodes[2].candidate_hash, candidate_a_hash);
		assert_eq!(tree.nodes[3].candidate_hash, candidate_b_hash);
		assert_eq!(tree.nodes[4].candidate_hash, candidate_a_hash);
	}

	#[test]
	fn hypothetical_depths_known_and_unknown() {
		let mut storage = CandidateStorage::new();

		let para_id = ParaId::from(5u32);
		let relay_parent_a = Hash::repeat_byte(1);

		let (pvd_a, candidate_a) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0a].into(),
			vec![0x0b].into(), // input same as output
			0,
		);
		let candidate_a_hash = candidate_a.hash();

		let (pvd_b, candidate_b) = make_committed_candidate(
			para_id,
			relay_parent_a,
			0,
			vec![0x0b].into(),
			vec![0x0a].into(), // input same as output
			0,
		);
		let candidate_b_hash = candidate_b.hash();

		let base_constraints = make_constraints(0, vec![0], vec![0x0a].into());

		let relay_parent_a_info = RelayChainBlockInfo {
			number: pvd_a.relay_parent_number,
			hash: relay_parent_a,
			storage_root: pvd_a.relay_parent_storage_root,
		};

		let max_depth = 4;
		storage.add_candidate(candidate_a, pvd_a).unwrap();
		storage.add_candidate(candidate_b, pvd_b).unwrap();
		let scope = Scope::with_ancestors(
			para_id,
			relay_parent_a_info,
			base_constraints,
			max_depth,
			vec![],
		)
		.unwrap();
		let tree = FragmentTree::populate(scope, &storage);

		let candidates: Vec<_> = tree.candidates().collect();
		assert_eq!(candidates.len(), 2);
		assert_eq!(tree.nodes.len(), max_depth + 1);

		assert_eq!(
			tree.hypothetical_depths(
				candidate_a_hash,
				HeadData::from(vec![0x0a]).hash(),
				relay_parent_a,
			),
			vec![0, 2, 4],
		);

		assert_eq!(
			tree.hypothetical_depths(
				candidate_b_hash,
				HeadData::from(vec![0x0b]).hash(),
				relay_parent_a,
			),
			vec![1, 3],
		);

		assert_eq!(
			tree.hypothetical_depths(
				CandidateHash(Hash::repeat_byte(21)),
				HeadData::from(vec![0x0a]).hash(),
				relay_parent_a,
			),
			vec![0, 2, 4],
		);

		assert_eq!(
			tree.hypothetical_depths(
				CandidateHash(Hash::repeat_byte(22)),
				HeadData::from(vec![0x0b]).hash(),
				relay_parent_a,
			),
			vec![1, 3]
		);
	}
}
