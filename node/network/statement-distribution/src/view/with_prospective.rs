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
use super::{StoredStatement, StoredStatementComparator, StatementFingerprint};

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

struct CandidateMetadata {
	para_id: ParaId,
	candidate_hash: CandidateHash,
	relay_parent: Hash,
	persisted_validation_data: PersistedValidationData,
}

// Data stored per candidate.
struct PerCandidate {
	metadata: CandidateMetadata,
	acceptance_status: AcceptanceStatus,

	// all the statements we've received about the candidate, stored in insertion order
	// so `Seconded` messages are first.
	statements: IndexMap<StoredStatementComparator, SignedFullStatement>,
}

enum AcceptanceStatus {
	Accepted, // by backing / prospective parachains.
	PendingAcceptance, // by backing / prospective parachains
}

/// Per-peer view of the protocol state.
#[derive(Default)]
pub(crate) struct PeerView {
	/// candidates that the peer is aware of because we sent statements to it. This indicates that we can
	/// send other statements pertaining to that candidate.
	sent_candidates: HashSet<CandidateHash>,
	/// candidates that peer is aware of, because we received statements from it.
	received_candidates: HashSet<CandidateHash>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to the peer by us.
	sent_statements: HashSet<StatementFingerprint>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to us by the peer.
	received_statements: HashSet<StatementFingerprint>,
	/// State which only relevant to particular relay-parents. This encompasses
	/// relay-parents in the implicit view as well.
	per_relay_parent: HashMap<Hash, PerRelayParentPeerView>,
}

struct PerRelayParentPeerView {
	/// How many large statements this peer already sent us.
	///
	/// Flood protection for large statements is rather hard and as soon as we get
	/// `https://github.com/paritytech/polkadot/issues/2979` implemented also no longer necessary.
	/// Reason: We keep messages around until we fetched the payload, but if a node makes up
	/// statements and never provides the data, we will keep it around for the slot duration. Not
	/// even signature checking would help, as the sender, if a validator, can just sign arbitrary
	/// invalid statements and will not face any consequences as long as it won't provide the
	/// payload.
	///
	/// Quick and temporary fix, only accept `MAX_LARGE_STATEMENTS_PER_SENDER` per connected node.
	///
	/// Large statements should be rare, if they were not, we would run into problems anyways, as
	/// we would not be able to distribute them in a timely manner. Therefore
	/// `MAX_LARGE_STATEMENTS_PER_SENDER` can be set to a relatively small number. It is also not
	/// per candidate hash, but in total as candidate hashes can be made up, as illustrated above.
	///
	/// An attacker could still try to fill up our memory, by repeatedly disconnecting and
	/// connecting again with new peer ids, but we assume that the resulting effective bandwidth
	/// for such an attack would be too low.
	large_statement_count: usize,

	/// We have seen a message that that is unexpected from this peer, so note this fact
	/// and stop subsequent logging and peer reputation flood.
	unexpected_count: usize,
}

impl PeerView {
	// TODO [now]: send/receive

	// TODO [now]: check_can_receive

	// TODO [now]: receive_large_statement?
}
