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
use std::time::Duration;

/// Default delay for sending reputation changes
pub const REPUTATION_CHANGE_INTERVAL: Duration = Duration::from_secs(30);

/// Collects reputation changes and sends them in one batch to relieve network channels
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
	/// New ReputationAggregator
	///
	/// # Arguments
	///
	/// * `send_immediately_if` - A function, takes `UnifiedReputationChange`,
	/// results shows if we need to send the changes right away.
	/// Useful for malicious changes and tests.
	pub fn new(send_immediately_if: fn(UnifiedReputationChange) -> bool) -> Self {
		Self { by_peer: Default::default(), send_immediately_if }
	}

	/// Sends collected reputation changes in a batch,
	/// removing them from inner state
	pub async fn send(
		&mut self,
		sender: &mut impl overseer::SubsystemSender<NetworkBridgeTxMessage>,
	) {
		sender
			.send_message(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Batch(
				self.by_peer.clone(),
			)))
			.await;

		self.by_peer.clear();
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
		let cost = rep.cost_or_benefit();
		self.by_peer
			.entry(peer_id)
			.and_modify(|v| *v = v.saturating_add(cost))
			.or_insert(cost);
	}
}
