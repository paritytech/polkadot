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

use std::collections::HashMap;

use polkadot_node_network_protocol::{OurView, PeerId, View};
use polkadot_primitives::v2::{CollatorPair, Id as ParaId, Hash, BlockNumber, SessionIndex};

use super::Metrics;

/// State for taking care of validator connections and advertisments.
struct Advertiser {
	/// Our network peer id.
	local_peer_id: PeerId,

	/// Our keys.
	collator_pair: CollatorPair,

	/// The para this collator is collating on.
	/// Starts as `None` and is updated with every `CollateOn` message.
	collating_on: Option<ParaId>,

	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our own view.
	view: OurView,

    /// The blocks we want to make sure to connect to validators for.
    required_connections: Option<ConnectionWindow>,

	/// Report metrics.
	metrics: Metrics,
}

/// Block heights we want established connections to validators for.
struct ConnectionWindow {
    /// Smalles block height, where we want to be connected already.
    ///
    /// Note that we will issue connection requests one block before already (if possible), but we
    /// will connect to the validators as of the given block height.
    connected_since: BlockNumber,
    /// Disconnect if we get a block with this height.
    connected_until: BlockNumber,
    /// The index of the session for this window.
    ///
    /// We invalidate connections on session changes.
    ///
    /// Note: With chain reversions it is possible that a block with the same height as another
    /// block, is in fact in a different session.
    current_session: SessionIndex,
}

impl Advertiser {
	fn new(local_peer_id: PeerId, collator_pair: CollatorPair, metrics: Metrics) -> Self {
		Self {
			local_peer_id,
			collator_pair,
			collating_on: None,
			peer_views: HashMap::new(),
			view: OurView::default(),
            required_connections: None,
			metrics,
		}
	}

	/// Get all peers which have the given relay parent in their view.
	fn peers_interested_in_leaf(&self, relay_parent: &Hash) -> Vec<PeerId> {
		self.peer_views
			.iter()
			.filter(|(_, v)| v.contains(relay_parent))
			.map(|(peer, _)| *peer)
			.collect()
	}

}
