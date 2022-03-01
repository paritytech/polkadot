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

//! A tree utility for managing parachain fragments unreferenced by the relay-chain.
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
//!   2. and candidates only refer to their parent by its head-data.
//!
//! The implication is that when we receive a candidate receipt, there are actually multiple
//! possibilities for any candidates between the para-head recorded in the relay parent's state
//! and the candidate in question.
//!
//! This means that our nodes need to handle multiple parents and that depth is an
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
// TODO [now]: review & update.

use std::{
	collections::{hash_map::Entry as HEntry, HashMap, HashSet, BTreeMap},
	sync::Arc,
};

use polkadot_node_subsystem::{
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext,
	SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	inclusion_emulator::staging::{
		ConstraintModifications, Constraints, Fragment, ProspectiveCandidate, RelayChainBlockInfo,
	},
	metrics::{self, prometheus},
};
use polkadot_primitives::vstaging::{
	Block, BlockId, BlockNumber, CandidateDescriptor, CandidateHash, CommittedCandidateReceipt,
	GroupIndex, GroupRotationInfo, Hash, HeadData, Header, Id as ParaId, PersistedValidationData,
	SessionIndex, ValidatorIndex,
};
use super::LOG_TARGET;

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
	) -> Result<(), PersistedValidationDataMismatch> {
		let candidate_hash = candidate.hash();

		if self.by_candidate_hash.contains_key(&candidate_hash) {
			return Ok(())
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

		Ok(())
	}

	// TODO [now]: fn restrict_to(&graphs) which will be our main pruning function
}

/// The state of a candidate.
///
/// Candidates aren't even considered until they've at least been seconded.
pub(crate) enum CandidateState {
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
pub(crate) struct Scope {
	para: ParaId,
	relay_parent: RelayChainBlockInfo,
	ancestors: BTreeMap<BlockNumber, (RelayChainBlockInfo, Constraints)>,
	ancestors_by_hash: HashSet<Hash>,
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
		ancestors: impl IntoIterator<Item=(RelayChainBlockInfo, Constraints)>,
	) -> Result<Self, UnexpectedAncestor> {
		let mut ancestors_map = BTreeMap::new();
		let mut ancestors_by_hash = HashSet::new();
		{
			let mut prev = relay_parent.number;
			for (ancestor, constraints) in ancestors {
				if prev == 0 {
					return Err(UnexpectedAncestor);
				} else if ancestor.number != prev - 1 {
					return Err(UnexpectedAncestor);
				} else if prev == base_constraints.min_relay_parent_number {
					break
				} else {
					prev = ancestor.number;
					ancestors_by_hash.insert(ancestor.hash);
					ancestors_map.insert(ancestor.number, (ancestor, constraints));
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
}

// We use indices into a flat vector to refer to nodes in the tree.
type NodePointer = usize;

/// This is a tree of candidates based on some underlying storage of candidates
/// and a scope.
pub(crate) struct FragmentTree {
	scope: Scope,
	nodes: Vec<FragmentNode>,
}

impl FragmentTree {
	/// Create a new [`FragmentTree`] with given scope, and populated from
	/// the provided node storage.
	pub fn new(scope: Scope, storage: &CandidateStorage) -> Self {
		let mut tree = FragmentTree {
			scope,
			nodes: Vec::new(),
		};

		tracing::trace!(
			target: LOG_TARGET,
			relay_parent = ?tree.scope.relay_parent.hash,
			relay_parent_num = tree.scope.relay_parent.number,
			para_id = ?tree.scope.para,
			ancestors = tree.scope.ancestors.len(),
			"Instantiating Fragment Tree",
		);

		// populate.

		tree
	}

	// TODO [now]: add new candidate and recursively populate as necessary.

	// TODO [now]: API for selecting backed candidates
}

struct FragmentNode {
	// A pointer to the parent node. `None` indicates that this is a root
	// node.
	parent: Option<NodePointer>,
	fragment: Fragment,
	erasure_root: Hash,
}

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
}

#[cfg(test)]
mod tests {
	// TODO [now]: scope rejects ancestors that skip blocks

	// TODO [now]: scope rejects ancestor of 0

	// TODO [now]: storage sets up links correctly.

	// TODO [now]: storage pruning.
}
