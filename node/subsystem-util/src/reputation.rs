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

use polkadot_node_network_protocol::{PeerId, UnifiedReputationChange};

#[derive(Debug, Clone)]
pub struct ReputationAggregator {
	malicious_reported: bool,
	by_peer: std::collections::HashMap<PeerId, i32>,
}

impl Default for ReputationAggregator {
	fn default() -> Self {
		Self::new()
	}
}

impl ReputationAggregator {
	pub fn new() -> Self {
		Self { malicious_reported: false, by_peer: std::collections::HashMap::new() }
	}

	pub fn clear(&mut self) {
		self.by_peer.clear();
	}

	pub fn update(&mut self, peer_id: PeerId, rep: UnifiedReputationChange) {
		if matches!(rep, UnifiedReputationChange::Malicious(_)) {
			self.malicious_reported = true;
		}
		let current = match self.by_peer.get(&peer_id) {
			Some(v) => *v,
			None => 0,
		};
		let new_value = current.saturating_add(rep.cost_or_benefit());
		self.by_peer.insert(peer_id, new_value);
	}

	pub fn malicious_reported(&self) -> bool {
		self.malicious_reported
	}

	pub fn by_peer(&self) -> &std::collections::HashMap<PeerId, i32> {
		&self.by_peer
	}
}
