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

use parity_scale_codec::{Encode, Decode};

use super::peer_set::PeerSet;
use super::{v1, View};

/// Messages for each protocol/peer set.
///
/// See also "super::peer_set".
#[derive(Debug)]
pub enum ProtocolMessage {
	/// Messages with regards to validation combined into a single protocol.
	Validation(WireMessage<v1::ValidationProtocol>),

	/// Messages with regards to collation combined into a single protocol.
	Collation(WireMessage<v1::CollationProtocol>),

	/// Availability distribution.
	///
	/// Actually part of validation, but needs to connect to far more nodes and is not gossip,
	/// therefore it is its own protocol.
	AvailabilityDistribution(WireMessage<v1::AvailabilityDistributionMessage>),
}

/// Messages are wrapped in `WireMessage` before being sent over the network.
///
/// This wrapping type makes it possible to send `ViewUpdate`s on every peer set. We bascially
/// extend every protocol we have with `ViewUpdates`.
///
/// I believe we will
/// want to get rid of this at some point and instead simply make `ViewUpdate` its own
/// protocol/peer set.
#[derive(Debug, Encode, Decode, Clone)]
pub enum WireMessage<M> {
	/// A message from a peer on a specific protocol.
	#[codec(index = "1")]
	ProtocolMessage(M),
	/// A view update from a peer.
	#[codec(index = "2")]
	ViewUpdate(View),
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
