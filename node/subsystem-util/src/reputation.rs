// Copyright (C) Parity Technologies (UK) Ltd.
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

use polkadot_node_network_protocol::{self as net_protocol, PeerId, UnifiedReputationChange};
use polkadot_node_subsystem::{messages::NetworkBridgeTxMessage, overseer};

/// TODO
#[derive(Debug, Clone)]
pub struct ReputationAggregator {
	send_immediately_if: fn(UnifiedReputationChange) -> bool,
	by_peer: std::collections::HashMap<PeerId, i32>,
}

impl Default for ReputationAggregator {
	fn default() -> Self {
		Self::new(|rep| matches!(rep, UnifiedReputationChange::Malicious(_)))
	}
}

impl ReputationAggregator {
	/// TODO
	pub fn new(send_immediately_if: fn(UnifiedReputationChange) -> bool) -> Self {
		Self { by_peer: Default::default(), send_immediately_if }
	}

	/// TODO
	pub async fn send(
		&mut self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
	) {
		for (&peer_id, &score) in &self.by_peer {
			sender
				.send_message(NetworkBridgeTxMessage::ReportPeer(
					peer_id,
					net_protocol::ReputationChange::new(score, "Aggregated reputation change"),
				))
				.await;
		}
		self.by_peer.clear();
	}

	/// TODO
	pub async fn modify(
		&mut self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
		peer_id: PeerId,
		rep: UnifiedReputationChange,
	) {
		self.add(peer_id, rep);
		if (self.send_immediately_if)(rep) {
			self.send(sender).await;
		}
	}

	fn add(&mut self, peer_id: PeerId, rep: UnifiedReputationChange) {
		let current = match self.by_peer.get(&peer_id) {
			Some(v) => *v,
			None => 0,
		};
		let new_value = current.saturating_add(rep.cost_or_benefit());
		self.by_peer.insert(peer_id, new_value);
	}
}
