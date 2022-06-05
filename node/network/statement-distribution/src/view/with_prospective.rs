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

use futures::{
	channel::{mpsc, oneshot},
	future::RemoteHandle,
	prelude::*,
};
use indexmap::IndexMap;

use polkadot_node_network_protocol::{
	self as net_protocol, PeerId, UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem::Span;
use polkadot_node_subsystem_util::backing_implicit_view::View as ImplicitView;
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, Id as ParaId,
	PersistedValidationData, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
	ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use crate::{LOG_TARGET, VC_THRESHOLD};
use super::{StoredStatement, StoredStatementComparator};

pub(crate) struct View {
	implicit_view: ImplicitView,
	per_active_leaf: HashMap<Hash, PerActiveLeaf>,
	candidate_store: CandidateStore,
}

impl View {
	/// Get a mutable handle to the implicit view.
	pub(crate) fn implicit_view_mut(&mut self) -> &mut ImplicitView {
		&mut self.implicit_view
	}

	/// Whether the view contains a given relay-parent.
	pub(crate) fn contains(&self, leaf_hash: &Hash) -> bool {
		// TODO [now]
		unimplemented!()
	}

	/// Deactivate the given leaf in the view, if it exists, and
	/// clean up after it.
	pub(crate) fn deactivate_leaf(&mut self, leaf_hash: &Hash) {
		self.implicit_view.deactivate_leaf(*leaf_hash);
		// TODO [now]: clean up un-anchored candidates in the store.
	}

	/// Activate the given relay-parent in the view. This overwrites
	/// any existing entry, and should only be called for fresh leaves.
	pub(crate) fn activate_leaf(
		&mut self,
		leaf_hash: Hash,
	) {
		// TODO [now] unimplemented
	}
}

struct CandidateStore {
	per_candidate: HashMap<CandidateHash, PerCandidate>,
}

// Data stored per active leaf.
struct PerActiveLeaf {
	live_candidates: HashMap<(ValidatorId, usize), Vec<CandidateHash>>,

	// Allowed relay-parents for each para.
	relay_parents_by_para: HashMap<ParaId, HashSet<Hash>>,
}

// Data stored per candidate.
struct PerCandidate {
	para_id: ParaId,
	candidate_hash: CandidateHash,
	relay_parent: Hash,
	persisted_validation_data: PersistedValidationData,
	acceptance_status: AcceptanceStatus,

	// stored in insertion order
	statements: IndexMap<StoredStatementComparator, SignedFullStatement>,
}

enum AcceptanceStatus {
	Accepted, // by backing / prospective parachains.
	PendingAcceptance, // by backing / prospective parachains
}

struct AcceptedCandidateStatements {

}
