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

//! Implementation of the v2 statement distribution protocol,
//! designed for asynchronous backing.

use polkadot_primitives::vstaging::{
	Hash, CandidateHash, CommittedCandidateReceipt, ValidatorId, SignedStatement, UncheckedSignedStatement,
	GroupIndex, PersistedValidationData,
};
use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::ValidationVersion,
	vstaging as protocol_vstaging, View, PeerId,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{CandidateBackingMessage, NetworkBridgeEvent, NetworkBridgeTxMessage},
	overseer, ActivatedLeaf, PerLeafSpan, StatementDistributionSenderTrait,
};
use polkadot_node_subsystem_util::backing_implicit_view::{FetchError, View as ImplicitView};

use sp_keystore::SyncCryptoStorePtr;

use std::collections::{HashMap, HashSet};

use crate::error::{JfyiError, JfyiErrorResult};

struct PerRelayParentState {
	max_seconded_count: usize,
	seconded_count: HashMap<ValidatorId, usize>,
	candidates: HashMap<CandidateHash, CandidateData>,
	known_by: HashSet<PeerId>,
}

struct CandidateData {
	state: CandidateState,
	statements: Vec<SignedStatement>,
}

enum CandidateState {
	/// The candidate is unconfirmed to exist, as it hasn't yet
	/// been fetched.
	Unconfirmed,
	/// The candidate is confirmed but we don't have the `PersistedValidationData`
	/// yet because we are missing some intermediate candidate.
	ConfirmedWithoutPVD(CommittedCandidateReceipt),
	/// The candidate is confirmed and we have the `PersistedValidationData`.
	Confirmed(CommittedCandidateReceipt, PersistedValidationData),
}

pub(crate) struct State {
	/// The utility for managing the implicit and explicit views in a consistent way.
	///
	/// We only feed leaves which have prospective parachains enabled to this view.
	implicit_view: ImplicitView,
	per_relay_parent: HashMap<Hash, PerRelayParentState>,
	peers: HashMap<PeerId, PeerState>,
	keystore: SyncCryptoStorePtr,
}

struct PeerState {
	view: View,
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_network_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: NetworkBridgeEvent<net_protocol::StatementDistributionMessage>,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer_id, _role, protocol_version, authority_ids) => {
			if protocol_version != ValidationVersion::VStaging.into() {
				return
			}

			state.peers.insert(peer_id, PeerState {
				view: View::default(),
			});

			// TODO [now]: update some authorities map.
		}
		NetworkBridgeEvent::PeerDisconnected(peer_id) => {
			state.peers.remove(&peer_id);
		}
		NetworkBridgeEvent::NewGossipTopology(new_topology) => {
			// TODO [now]
		}
		NetworkBridgeEvent::PeerMessage(peer_id, message) => {
			match message {
				net_protocol::StatementDistributionMessage::V1(_) => return,
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::V1Compatibility(_)
				) => return,
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, statement)
				) => {} // TODO [now]
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::BackedCandidateInventory(inner)
				) => {} // TODO [now]
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(relay_parent, candidate_hash)
				) => {} // TODO [now]
			}
		}
		NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
			// TODO [now]
		}
		NetworkBridgeEvent::OurViewChange(_view) => {
			// handled by `handle_activated_leaf`
		}
	}
}

/// This should only be invoked for leaves that implement prospective parachains.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn activate_leaf<Context>(
	ctx: &mut Context,
	state: &mut State,
	leaf: ActivatedLeaf,
) -> JfyiErrorResult<()> {

	state.implicit_view.activate_leaf(ctx.sender(), leaf.hash)
		.await
		.map_err(JfyiError::ActivateLeafFailure)?;

	for leaf in state.implicit_view.all_allowed_relay_parents() {
		if state.per_relay_parent.contains_key(leaf) { continue }

		// TODO [now]:
		//   1. fetch info about validators, groups, and assignments
		//   2. initialize PerRelayParentState
		//   3. try to find new commonalities with peers and send data to them.
		state.per_relay_parent.insert(
			*leaf,
			PerRelayParentState {
				max_seconded_count: unimplemented!(),
				seconded_count: HashMap::new(),
				candidates: HashMap::new(),
				known_by: HashSet::new(),
			}
		);
	}

	Ok(())
}

pub(crate) fn deactivate_leaf(
	state: &mut State,
	leaf_hash: Hash,
) {
	// deactivate the leaf in the implicit view.
	state.implicit_view.deactivate_leaf(leaf_hash);
	let relay_parents = state.implicit_view.all_allowed_relay_parents().collect::<HashSet<_>>();

	// fast exit for no-op.
	if relay_parents.len() == state.per_relay_parent.len() { return }

	// clean up per-relay-parent data based on everything removed.
	state.per_relay_parent.retain(|r, _| relay_parents.contains(r));
}
