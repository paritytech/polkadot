// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

pub use sc_network::{PeerId, ReputationChange};

use polkadot_node_network_protocol::{peer_set::PeerSet, v1::ValidationProtocol, v1::CollationProtocol, message::ProtocolMessage, ObservedRole, OurView, View};
use super::AllMessages;

/// Generic events from network.
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkBridgeEvent {
	/// A peer has connected.
	PeerConnected(PeerId, ObservedRole),

	/// A peer has disconnected.
	PeerDisconnected(PeerId),

	/// Peer's `View` has changed.
	PeerViewChange(PeerId, View),

	/// Our view has changed.
	OurViewChange(OurView),
}

impl NetworkBridgeEvent {
    /// Create specific events/messages for all subsystems interested in `NetworkBridgeEvent`s from a given peer set.
    pub fn distribute_to_subsystems(self, peer_set: PeerSet) -> Vec<AllMessages> {
        match  peer_set {
            PeerSet::Validation =>
                vec![
                    AllMessages::BitfieldDistribution(self.clone().into()),
                    AllMessages::PoVDistribution(self.clone().into()),
                    AllMessages::StatementDistribution(self.clone().into()),
                    AllMessages::AvailabilityRecovery(self.into()),
                ],
            PeerSet::Collation => 
                vec![
                    AllMessages::CollatorProtocol(self.into()),
                ],
            PeerSet::AvailabilityDistribution =>
                vec![
                   AllMessages::AvailabilityDistribution(self.into()),
                ]
        }
    }
}

/// Route a protocol messages to the concerned subsystems.
///
/// The returned `AllMessages` is suitable to be sent over the overseer. Right now all protocol
/// messages are only destined to exactly one particular subsystem. If that changes at any point
/// (one network message should get delivered to multiple subsystems), the return value can be made
/// a `Vec` again, containing all the messages that need to be sent.
pub fn route_protocol_message(msg: ProtocolMessage) -> AllMessages {
    match msg {
        ProtocolMessage::Validation(p) =>
            route_validation_message(p),
        ProtocolMessage::Collation(CollationProtocol::CollatorProtocol(p)) =>
            AllMessages::CollatorProtocol(p.into()),
        ProtocolMessage::AvailabilityDistribution(p) =>
            AllMessages::AvailabilityDistribution(p.into()),
    }
}

/// Route messages in the ValidationProtocol to interested subsystems.
pub fn route_validation_message(p: ValidationProtocol) -> AllMessages {
    match p {
        ValidationProtocol::BitfieldDistribution(msg) =>
            AllMessages::BitfieldDistribution(msg.into()),
        ValidationProtocol::PoVDistribution(msg) =>
            AllMessages::PoVDistribution(msg.into()),
        ValidationProtocol::StatementDistribution(msg) =>
            AllMessages::StatementDistribution(msg.into()),
        ValidationProtocol::AvailabilityRecovery(msg) =>
            AllMessages::AvailabilityRecovery(msg.into()),
    }
}
