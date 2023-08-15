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

//! A utility abstraction to collect and send reputation changes.

use polkadot_node_network_protocol::{PeerId, UnifiedReputationChange};
use polkadot_node_subsystem::{
	messages::{NetworkBridgeTxMessage, ReportPeerMessage},
	overseer,
};
use std::{collections::HashMap, time::Duration};

/// Default delay for sending reputation changes
pub const REPUTATION_CHANGE_INTERVAL: Duration = Duration::from_secs(30);

type BatchReputationChange = HashMap<PeerId, i32>;

/// Collects reputation changes and sends them in one batch to relieve network channels
#[derive(Debug, Clone)]
pub struct ReputationAggregator {
	send_immediately_if: fn(UnifiedReputationChange) -> bool,
	by_peer: Option<BatchReputationChange>,
}

impl Default for ReputationAggregator {
	fn default() -> Self {
		Self::new(|rep| matches!(rep, UnifiedReputationChange::Malicious(_)))
	}
}

impl ReputationAggregator {
	/// New `ReputationAggregator`
	///
	/// # Arguments
	///
	/// * `send_immediately_if` - A function, takes `UnifiedReputationChange`,
	/// results shows if we need to send the changes right away.
	/// By default, it is used for sending `UnifiedReputationChange::Malicious` changes immediately
	/// and for testing.
	pub fn new(send_immediately_if: fn(UnifiedReputationChange) -> bool) -> Self {
		Self { by_peer: Default::default(), send_immediately_if }
	}

	/// Sends collected reputation changes in a batch,
	/// removing them from inner state
	pub async fn send(
		&mut self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
	) {
		if let Some(by_peer) = self.by_peer.take() {
			sender
				.send_message(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Batch(by_peer)))
				.await;
		}
	}

	/// Adds reputation change to inner state
	/// or sends it right away if the change is dangerous
	pub async fn modify(
		&mut self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
		peer_id: PeerId,
		rep: UnifiedReputationChange,
	) {
		if (self.send_immediately_if)(rep) {
			self.single_send(sender, peer_id, rep).await;
		} else {
			self.add(peer_id, rep);
		}
	}

	async fn single_send(
		&self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
		peer_id: PeerId,
		rep: UnifiedReputationChange,
	) {
		sender
			.send_message(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(
				peer_id,
				rep.into(),
			)))
			.await;
	}

	fn add(&mut self, peer_id: PeerId, rep: UnifiedReputationChange) {
		if self.by_peer.is_none() {
			self.by_peer = Some(HashMap::new());
		}
		if let Some(ref mut by_peer) = self.by_peer {
			add_reputation(by_peer, peer_id, rep)
		}
	}
}

/// Add a reputation change to an existing collection.
pub fn add_reputation(
	acc: &mut BatchReputationChange,
	peer_id: PeerId,
	rep: UnifiedReputationChange,
) {
	let cost = rep.cost_or_benefit();
	acc.entry(peer_id).and_modify(|v| *v = v.saturating_add(cost)).or_insert(cost);
}
