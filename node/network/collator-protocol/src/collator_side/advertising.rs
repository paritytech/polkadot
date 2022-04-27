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

use std::collections::{HashMap, HashSet};

use polkadot_node_network_protocol::v1::CollatorProtocolMessage;
use polkadot_node_network_protocol::{OurView, PeerId, View};
use polkadot_primitives::v2::{CollatorPair, Id as ParaId, Hash, BlockNumber, SessionIndex, GroupRotationInfo, CoreIndex, GroupIndex, AuthorityDiscoveryId};
use polkadot_subsystem::messages::CollatorProtocolMessage;
use polkadot_subsystem::{ActiveLeavesUpdate, SubsystemContext, SubsystemSender};

use crate::error::FatalResult;

use crate::error::FatalResult;

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

    /// Information about connections we want to have established.
    ///
    /// For connection management we basically need two blocks of information:
    ///
    /// 1. Time: When and for how long do we want the connection.
    /// 2. To what validators/which backing group to connect to.
    ///
    /// ## Connection time management
    ///
    /// For simplicity we chose to make connection life management based on relay chain blocks,
    /// which act as a natural pace maker. So the lifespan of a connection can be adjusted in
    /// multiples of rougly 6 seconds (assuming normal operation).
    ///
    /// To ensure uninterrupted connectivity, for several blocks, all you have to do is to ensure
    /// that this map contains an entry for those consecutive block numbers. We chose block numbers
    /// as key for this map as opposed to `Hash`es, so you can ensure "guaranteed" uninterrupted
    /// connectivity from a current block to a future block, e.g. for pre-connect.
    ///
    /// Concretely: For pre-connect (establishing a connection one block earlier than you need it),
    /// you would make sure that this map contains two entries with the same value, one for the
    /// current block height of the fork you are interested in and one with the block number
    /// incremented. This way the connection will survive the next block in all cases.
    ///
    /// # Target/what validators to connect to
    ///
	/// The values of this map are the group/session combinations we want to be connected for the
	/// given block height.
    required_connections: HashMap<BlockNumber, HashSet<(SessionIndex, GroupIndex)>>,

	/// Information about established connections to validators in a group.
	connections: HashMap<(SessionIndex, GroupIndex), GroupConnection>,

	/// Lookup group index/session indexes for a peer that connected.
	reverse_peers: HashMap<PeerId, HashSet<(SessionIndex, GroupIndex)>>,

	/// Lookup groups in sessions for an Authority (in requested connections)
	authority_group: HashMap<AuthorityDiscoveryId, HashSet<(SessionIndex, GroupIndex)>>,

	// send task (given relay_parent, paraid):
	//	- send for paraid
	//	- resolves to coreid (on a per block basis) - can be found once we have block
	//	- resolves to groupid - can be found once we know the session (and block height)
	//	- resolves to `AuthorityDiscoveryIds` - can be found if we have session
	//	- which resolve to `PeerId`s - can be resolved once connected
	requested_connections: HashMap<AuthorityDiscoveryId, GroupIndex>,
	/// `PeerId`s for already established connections for some group.
	group_peers: HashMap<GroupIndex, HashSet<PeerId>>,

	established_connections: HashMap<Hash, GroupIndex>,

	/// Report metrics.
	metrics: Metrics,
}

/// Needed information for establishing connections.
struct ConnectionInfo {
	/// The relay parent to use for determining the correct core.
	relay_parent: Hash,
	/// Block number to determine the desired backing group.
	///
	/// Note: This seemingly redundant info, is not redundant in the case of `pre-connect`. In this
	/// case the above relay_parent will be used for determining the core in advance, but the
	/// responsbile backing group will be determined based on the given `block_number`.
	block_number: BlockNumber,
	/// What session we are operating in.
	///
	/// For `pre-connect`, this will just be a guess, which might be wrong (we assume to stay in
	/// the same session).
	session_index: SessionIndex,
}

/// Information about connections to a validator group.
///
/// We keep track of connected peers in a validator group and the messages that should be sent to
/// them.
struct GroupConnection {
	/// Connected peers.
	peers: HashSet<PeerId>,
	/// Messages that should be sent to connected peers in this group.
	///
	/// When messages are sent, peers might not yet be connected, so we keep all messages that
	/// should be sent to peers here, if a new peer connects all messages that are still relevant
	/// to its view are sent.
	messages: Vec<CollatorProtocolMessage>,
}

impl Advertiser {
	pub fn new(local_peer_id: PeerId, collator_pair: CollatorPair, metrics: Metrics) -> Self {
		Self {
			local_peer_id,
			collator_pair,
			collating_on: None,
			peer_views: HashMap::new(),
			view: OurView::default(),
            required_connections: HashMap::new(),
			metrics,
		}
	}

	/// Ask for additional connections.
	///
	/// They will automatically established once a block with block height `when` comes into view
	/// and will be closed, once it goes out of view, assuming no other entry is present, which
	/// preserves them.
	///
	// - Lookup SessionIndex & Group
	// - Add to required_connections
	// - Update Connect message if necessary (connection affects already present block heights)
	pub fn add_required_connections<Sender: SubsystemSender>(&mut self, sender: &mut Sender, when: BlockNumber, info: ConnectionInfo) {
		self.requried_connections.insert(when, info);
	}

	/// Send a message to a validator group.
	//
	// - insert message
	// - send to all already connected peers if in view.
	//		-> View still relevant with async backing? Likely only when it comes to height.
	pub fn send_message(&mut self, session: SessionIndex, group: GroupIndex, msg: CollatorProtocolMessage) -> FatalResult<()> {
		panic!("WIP");
	}

	// - Add to peers in groups.
	// - Send any messages it has not received yet.
	// - Cleanout any obsolete messages
	pub fn on_peer_connected(&mut self, ...);


	/// Process an active leaves update.
	///
	/// - Make sure needed connections are established
	/// - Make sure obsolete connections are dropped
    pub fn process_active_leaves_update<Context: SubsystemContext>(
            &mut self,
            ctx: &mut Context,
            update: &ActiveLeavesUpdate,
            ) -> FatalResult<()> {
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
