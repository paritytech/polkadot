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

//! A graph utility for managing unbacked parachain fragments.
//!
//! Each node in the graph represents a candidate. Nodes do not uniquely refer to a parachain
//! block for two reasons.
//!   1. There's no requirement that head-data is unique
//!      for a parachain. Furthermore, a parachain is under no obligation to be acyclic, and this is mostly
//!      just because it's totally inefficient to enforce it. Practical use-cases are acyclic, but there is
//!      still more than one way to reach the same head-data.
//!   2. and candidates only refer to their parent by its head-data.
//!
//! The implication is that when we receive a candidate receipt, there are actually multiple
//! possibilities for any candidates between the para-head recorded in the relay parent's state
//! and the candidate we're examining.
//!
//! This means that our nodes need to handle multiple parents and that depth is an
//! attribute of a path, not a candidate.
//!
//! We also need to handle cycles, including nodes for candidates which produce a header
//! which is the same as its parent's.
//!
//! The graph exposes a 'frontier' of nodes which appear to be the best to build upon
//! and is the primary means for higher-level code to select candidates to build upon.

use std::{
	collections::{hash_map::Entry as HEntry, HashMap, HashSet},
	sync::Arc,
};

use polkadot_node_subsystem::{
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext,
	SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	inclusion_emulator::staging::{
		ConstraintModifications, Constraints, Fragment, RelayChainBlockInfo,
	},
	metrics::{self, prometheus},
};
use polkadot_primitives::vstaging::{
	Block, BlockId, BlockNumber, CandidateDescriptor, CandidateHash, CommittedCandidateReceipt,
	GroupIndex, GroupRotationInfo, Hash, Header, Id as ParaId, SessionIndex, ValidatorIndex,
};

// TODO [now]: separate graph per relay-parent (constraints)?
// TODO [now]: keep nodes and graphs separate? recompute / prune graphs
//             on every new relay parent?
// TODO [now]: API for selecting backed candidates
pub(crate) struct FragmentGraph {
	para: ParaId,
	relay_parent: RelayChainBlockInfo,
	base_constraints: Constraints,
}

struct CandidateGraph {
	// TODO [now]: semi-ordered pile of candidates.
	// we'll need to support some kinds of traversal and insertions
}

enum FragmentState {
	// The fragment has been seconded.
	Seconded,
	// The fragment has been completely backed by the group.
	Backed,
}

struct FragmentNode {
	// The hash of the head-data of the parent node
	parent: Hash,
	// Candidate hashes of children.
	children: Vec<CandidateHash>,
	fragment: Fragment,
	erasure_root: Hash,
	state: FragmentState,
}

impl FragmentNode {
	fn relay_parent(&self) -> Hash {
		self.fragment.relay_parent().hash
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
