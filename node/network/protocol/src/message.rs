// Copyright 2021 Parity Technologies (UK) Ltd.
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

use super::peer_set::PeerSet;
use super::v1;

/// Messages for each protocol/peer set.
///
/// See also "super::peer_set".
pub enum ProtocolMessage {
	/// Messages with regards to validation combined into a single protocol.
	Validation(v1::ValidationProtocol),

	/// Messages with regards to collation combined into a single protocol.
	Collation(v1::CollationProtocol),

	/// Availability distribution.
	///
	/// Actually part of validation, but needs to connect to far more nodes and is not gossip,
	/// therefore it is its own protocol.
	AvailabilityDistribution(v1::AvailabilityDistributionMessage),
}

impl ProtocolMessage {
	/// Get the peer set corresponding to this protocol.
	pub fn get_peer_set(&self) -> PeerSet {
		match self {
			ProtocolMessage::Validation(_) => PeerSet::Validation,
			ProtocolMessage::Collation(_) => PeerSet::Collation,
			ProtocolMessage::AvailabilityDistribution(_) => PeerSet::AvailabilityDistribution,
		}
	}
}
