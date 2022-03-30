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

//! The state used by collator side for tracking preconnect requests.

use std::collections::{HashMap, HashSet, VecDeque};

use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::v2::{AuthorityDiscoveryId, Hash, SessionIndex};

/// Preconnect state for particular relay parent.
#[derive(Debug)]
pub struct PerRelayParent {
	/// Assumed session index used for determining validators group.
	/// It's necessary to store it in order to be able to discard invalid
	/// preconnected group at session boundaries.
	pub session_index: SessionIndex,

	/// Discovery IDs of all validators from the group.
	pub validators: HashSet<AuthorityDiscoveryId>,

	/// Peers we have established connection with.
	pub connected: HashSet<PeerId>,
}

#[derive(Debug, Default)]
pub struct PreconnectState {
	/// A queue of relay parents and their corresponding states.
	///
	/// Since a candidate executed in the context of the block `R` must
	/// be backed by the group assigned to `number(R) + 1`, this queue stores 2
	/// items at most:
	///
	/// - Possibly one for parent of `R` for candidates based on `R`
	/// - Possibly one for `R` for candidates based on child of `R`
	///
	/// (assuming `R` is the latest observed relay block)
	per_relay_parent: VecDeque<(Hash, PerRelayParent)>,
}

impl PreconnectState {
	/// Get a preconnect state for the given relay parent.
	pub fn get(&self, relay_parent: &Hash) -> Option<&PerRelayParent> {
		self.per_relay_parent
			.iter()
			.find(|(hash, _)| hash == relay_parent)
			.map(|(_, state)| state)
	}

	/// Save a new preconnect state, does nothing if the relay parent is already present
	/// in the queue.
	///
	/// Returns `false` if the parent was already queued and `true` otherwise.
	pub fn insert(
		&mut self,
		relay_parent: &Hash,
		session_index: SessionIndex,
		authority_ids: impl IntoIterator<Item = AuthorityDiscoveryId>,
		connected_peers: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
	) -> bool {
		if self.per_relay_parent.iter().any(|(hash, _)| hash == relay_parent) {
			return false
		}

		let authority_ids: HashSet<_> = authority_ids.into_iter().collect();
		let mut connected = HashSet::new();

		// Collators may happen to collate multiple times in a row on a single
		// parachain, in case there's no group rotation, validators are expected
		// to keep the connection alive.
		for (peer_id, peer_discovery_ids) in connected_peers {
			if authority_ids.intersection(peer_discovery_ids).next().is_some() {
				connected.insert(peer_id.clone());
			}
		}

		self.per_relay_parent.push_back((
			*relay_parent,
			PerRelayParent { session_index, validators: authority_ids, connected },
		));

		true
	}

	/// Drop all queued preconnect requests until the given relay parent, exclusive.
	pub fn clear_until(&mut self, relay_parent: &Hash) {
		self.per_relay_parent = self
			.per_relay_parent
			.drain(..)
			.skip_while(|(hash, _)| hash != relay_parent)
			.collect();
	}

	/// Note a new peer connected.
	pub fn on_peer_connected(
		&mut self,
		peer_id: &PeerId,
		authority_ids: &HashSet<AuthorityDiscoveryId>,
	) {
		self.per_relay_parent.iter_mut().for_each(|(_, state)| {
			// One peer id may correspond to different discovery ids across sessions,
			// but the opposite is not true. Thus, having non-empty intersection is
			// sufficient to say that this peer is the one we are looking for.
			if state.validators.intersection(authority_ids).next().is_some() {
				state.connected.insert(peer_id.clone());
			}
		})
	}

	/// Drop disconnected peer from the state.
	pub fn on_peer_disconnected(&mut self, peer_id: &PeerId) {
		self.per_relay_parent.iter_mut().for_each(|(_, state)| {
			state.connected.remove(peer_id);
		});
	}
}
