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
	BlockNumber, CandidateDescriptor, CandidateHash, CommittedCandidateReceipt, Hash, HeadData,
	Id as ParaId, PersistedValidationData,
};

/// An error indicating that a supplied candidate didn't match the persisted
/// validation data provided alongside it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PersistedValidationDataMismatch;

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
	) -> Result<CandidateHash, PersistedValidationDataMismatch> {
		let candidate_hash = candidate.hash();

		if self.by_candidate_hash.contains_key(&candidate_hash) {
			return Ok(candidate_hash)
		}

		if persisted_validation_data.hash() != candidate.descriptor.persisted_validation_data_hash {
			return Err(PersistedValidationDataMismatch)
		}

		let parent_head_hash = persisted_validation_data.parent_head.hash();

		let entry = CandidateEntry {
			candidate_hash,
			relay_parent: candidate.descriptor.relay_parent,
			erasure_root: candidate.descriptor.erasure_root,
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
	erasure_root: Hash,
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

	fn earliest_relay_parent(&self) -> RelayChainBlockInfo {
		self.ancestors
			.iter()
			.next()
			.map(|(_, v)| v.clone())
			.unwrap_or_else(|| self.relay_parent.clone())
	}

	fn ancestor_by_hash(&self, hash: &Hash) -> Option<RelayChainBlockInfo> {
		if hash == &self.relay_parent.hash {
			return Some(self.relay_parent.clone())
		}

		self.ancestors_by_hash.get(hash).map(|info| info.clone())
	}
}

// We use indices into a flat vector to refer to nodes in the tree.
// Every tree also has an implicit root.
#[derive(Debug, Clone, Copy, PartialEq)]
enum NodePointer {
	Root,
	Storage(usize),
}

/// Abstraction around `&'a CandidateStorage`.
trait PopulateFrom<'a> {
	type ParaChildrenIter: Iterator<Item = &'a CandidateEntry> + 'a;

	fn get(&self, candidate_hash: &CandidateHash) -> Option<&'a CandidateEntry>;

	fn iter_para_children(&self, parent_head_hash: &Hash) -> Self::ParaChildrenIter;
}

impl<'a> PopulateFrom<'a> for &'a CandidateStorage {
	type ParaChildrenIter = Box<dyn Iterator<Item = &'a CandidateEntry> + 'a>;

	fn get(&self, candidate_hash: &CandidateHash) -> Option<&'a CandidateEntry> {
		CandidateStorage::get(self, candidate_hash)
	}

	fn iter_para_children(&self, parent_head_hash: &Hash) -> Self::ParaChildrenIter {
		Box::new(CandidateStorage::iter_para_children(self, parent_head_hash))
	}
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

	// Inserts a node and updates child references in a non-root parent.
	fn insert_node(&mut self, node: FragmentNode) {
		let pointer = NodePointer::Storage(self.nodes.len());
		let parent_pointer = node.parent;
		let candidate_hash = node.candidate_hash;

		let max_depth = self.scope.max_depth;

		self.candidates
			.entry(candidate_hash)
			.or_insert_with(|| bitvec![u16, Msb0; 0; max_depth])
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

	/// Add a candidate and recursively populate from storage.
	pub(crate) fn add_and_populate(&mut self, hash: CandidateHash, storage: &CandidateStorage) {
		self.add_and_populate_from(hash, storage);
	}

	/// Returns the hypothetical depths where a candidate with the given hash and parent head data
	/// would be added to the tree.
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
			if let Some(info) = self.scope.ancestors_by_hash.get(&candidate_relay_parent) {
				info.number
			} else {
				return Vec::new()
			};

		let max_depth = self.scope.max_depth;
		let mut depths = bitvec![u16, Msb0; 0; max_depth];

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

	fn add_and_populate_from<'a>(&mut self, hash: CandidateHash, storage: impl PopulateFrom<'a>) {
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
		storage: impl PopulateFrom<'a>,
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

				if child_depth >= self.scope.max_depth {
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
							relay_parent,
							child_constraints.clone(),
							candidate.candidate.clone(),
						);

						match f {
							Ok(f) => f,
							Err(_) => continue,
						}
					};

					let mut cumulative_modifications = modifications.clone();
					cumulative_modifications.stack(fragment.constraint_modifications());

					let head_data_hash = fragment.candidate().commitments.head_data.hash();
					let node = FragmentNode {
						parent: parent_pointer,
						fragment,
						erasure_root: candidate.erasure_root.clone(),
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

#[allow(unused)] // TODO [now]
struct FragmentNode {
	// A pointer to the parent node.
	parent: NodePointer,
	fragment: Fragment,
	erasure_root: Hash,
	candidate_hash: CandidateHash,
	depth: usize,
	cumulative_modifications: ConstraintModifications,
	head_data_hash: Hash,
	children: Vec<(NodePointer, CandidateHash)>,
}

#[allow(unused)] // TODO [now]
impl FragmentNode {
	fn relay_parent(&self) -> Hash {
		self.fragment.relay_parent().hash
	}

	fn parent_head_data(&self) -> &HeadData {
		&self.fragment.candidate().persisted_validation_data.parent_head
	}

	/// Produce a candidate receipt from this fragment node.
	fn produce_candidate_receipt(&self, para_id: ParaId) -> CommittedCandidateReceipt {
		let candidate = self.fragment.candidate();

		CommittedCandidateReceipt {
			commitments: candidate.commitments.clone(),
			descriptor: CandidateDescriptor {
				para_id,
				relay_parent: self.relay_parent(),
				collator: candidate.collator.clone(),
				signature: candidate.collator_signature.clone(),
				persisted_validation_data_hash: candidate.persisted_validation_data.hash(),
				pov_hash: candidate.pov_hash,
				erasure_root: self.erasure_root,
				para_head: candidate.commitments.head_data.hash(),
				validation_code_hash: candidate.validation_code_hash.clone(),
			},
		}
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
	use polkadot_primitives::vstaging::{CandidateCommitments, CandidateDescriptor};
	use polkadot_primitives_test_helpers as test_helpers;

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
		let base_constraints = Constraints {
			min_relay_parent_number: 8,
			max_pov_size: 1_000_000,
			max_code_size: 1_000_000,
			ump_remaining: 10,
			ump_remaining_bytes: 1_000,
			dmp_remaining_messages: 10,
			hrmp_inbound: InboundHrmpLimitations { valid_watermarks: vec![8, 9] },
			hrmp_channels_out: HashMap::new(),
			max_hrmp_num_per_candidate: 0,
			required_parent: HeadData(vec![1, 2, 3]),
			validation_code_hash: Hash::repeat_byte(69).into(),
			upgrade_restriction: None,
			future_validation_code: None,
		};

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
		let base_constraints = Constraints {
			min_relay_parent_number: 0,
			max_pov_size: 1_000_000,
			max_code_size: 1_000_000,
			ump_remaining: 10,
			ump_remaining_bytes: 1_000,
			dmp_remaining_messages: 10,
			hrmp_inbound: InboundHrmpLimitations { valid_watermarks: vec![8, 9] },
			hrmp_channels_out: HashMap::new(),
			max_hrmp_num_per_candidate: 0,
			required_parent: HeadData(vec![1, 2, 3]),
			validation_code_hash: Hash::repeat_byte(69).into(),
			upgrade_restriction: None,
			future_validation_code: None,
		};

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
		let base_constraints = Constraints {
			min_relay_parent_number: 3,
			max_pov_size: 1_000_000,
			max_code_size: 1_000_000,
			ump_remaining: 10,
			ump_remaining_bytes: 1_000,
			dmp_remaining_messages: 10,
			hrmp_inbound: InboundHrmpLimitations { valid_watermarks: vec![8, 9] },
			hrmp_channels_out: HashMap::new(),
			max_hrmp_num_per_candidate: 0,
			required_parent: HeadData(vec![1, 2, 3]),
			validation_code_hash: Hash::repeat_byte(69).into(),
			upgrade_restriction: None,
			future_validation_code: None,
		};

		let scope =
			Scope::with_ancestors(para_id, relay_parent, base_constraints, max_depth, ancestors)
				.unwrap();

		assert_eq!(scope.ancestors.len(), 2);
		assert_eq!(scope.ancestors_by_hash.len(), 2);
	}

	#[test]
	fn storage_add_candidate() {
		let mut storage = CandidateStorage::new();
		let persisted_validation_data = PersistedValidationData {
			parent_head: vec![4, 5, 6].into(),
			relay_parent_number: 8,
			relay_parent_storage_root: Hash::repeat_byte(69),
			max_pov_size: 1_000_000,
		};

		let candidate = CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: ParaId::from(5u32),
				relay_parent: Hash::repeat_byte(69),
				collator: test_helpers::dummy_collator(),
				persisted_validation_data_hash: persisted_validation_data.hash(),
				pov_hash: Hash::repeat_byte(1),
				erasure_root: Hash::repeat_byte(1),
				signature: test_helpers::dummy_collator_signature(),
				para_head: Hash::repeat_byte(1),
				validation_code_hash: Hash::repeat_byte(1).into(),
			},
			commitments: CandidateCommitments {
				upward_messages: Vec::new(),
				horizontal_messages: Vec::new(),
				new_validation_code: None,
				head_data: vec![1, 2, 3].into(),
				processed_downward_messages: 0,
				hrmp_watermark: 10,
			},
		};
		let candidate_hash = candidate.hash();
		let parent_head_hash = persisted_validation_data.parent_head.hash();

		storage.add_candidate(candidate, persisted_validation_data).unwrap();
		assert!(storage.contains(&candidate_hash));
		assert_eq!(storage.iter_para_children(&parent_head_hash).count(), 1);
	}

	// TODO [now]: retain

	// TODO [now]: recursive populate

	// TODO [now]: enforce root-child nodes contiguous

	// TODO [now]: add candidate child of root

	// TODO [now]: add candidate child of non-root

	// TODO [now]: hypothetical_depths for existing candidate.

	// TODO [now]: hypothetical_depths for non-existing candidate based on root.

	// TODO [now]: hypothetical_depths for non-existing candidate not based on root.
}
