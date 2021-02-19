// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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
//

use futures::channel::mpsc;

use parity_scale_codec::Decode;
use polkadot_node_network_protocol::{
	peer_set::PeerSet, v1 as protocol_v1, PeerId, UnifiedReputationChange,
};
use polkadot_primitives::v1::{AuthorityDiscoveryId, BlockNumber};
use polkadot_subsystem::messages::{AllMessages, NetworkBridgeMessage};
use polkadot_subsystem::{ActiveLeavesUpdate, FromOverseer, OverseerSignal};
use sc_network::Event as NetworkEvent;

use polkadot_node_network_protocol::{request_response::Requests, ObservedRole};

use super::multiplexer::RequestMultiplexError;
use super::{WireMessage, MALFORMED_MESSAGE_COST};

/// Internal type combining all actions a `NetworkBridge` might perform.
///
/// Both messages coming from the network (`NetworkEvent`) and messages coming from other
/// subsystems (`FromOverseer`) will be converted to `Action` in `run_network` before being
/// processed.
#[derive(Debug)]
pub(crate) enum Action {
	/// Ask network to send a validation message.
	SendValidationMessages(Vec<(Vec<PeerId>, protocol_v1::ValidationProtocol)>),

	/// Ask network to send a collation message.
	SendCollationMessages(Vec<(Vec<PeerId>, protocol_v1::CollationProtocol)>),

	/// Ask network to send requests.
	SendRequests(Vec<Requests>),

	/// Ask network to connect to validators.
	ConnectToValidators {
		validator_ids: Vec<AuthorityDiscoveryId>,
		peer_set: PeerSet,
		connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	},

	/// Report a peer to the network implementation (decreasing/increasing its reputation).
	ReportPeer(PeerId, UnifiedReputationChange),

	/// A subsystem updates us on the relay chain leaves we consider active.
	///
	/// Implementation will send `WireMessage::ViewUpdate` message to peers as appropriate to the
	/// change.
	ActiveLeaves(ActiveLeavesUpdate),

	/// A subsystem updates our view on the latest finalized block.
	///
	/// This information is used for view updates, see also `ActiveLeaves`.
	BlockFinalized(BlockNumber),

	/// Network tells us about a new peer.
	PeerConnected(PeerSet, PeerId, ObservedRole),

	/// Network tells us about a peer that left.
	PeerDisconnected(PeerSet, PeerId),

	/// Messages from the network targeted to other subsystems.
	PeerMessages(
		PeerId,
		Vec<WireMessage<protocol_v1::ValidationProtocol>>,
		Vec<WireMessage<protocol_v1::CollationProtocol>>,
	),

	/// Send a message to another subsystem or the overseer.
	///
	/// Used for handling incoming requests.
	SendMessage(AllMessages),

	/// Abort with reason.
	Abort(AbortReason),
	Nop,
}

#[derive(Debug)]
pub(crate) enum AbortReason {
	/// Received error from overseer:
	SubsystemError(polkadot_subsystem::SubsystemError),
	/// The stream of incoming events concluded.
	EventStreamConcluded,
	/// The stream of incoming requests concluded.
	RequestStreamConcluded,
	/// We received OverseerSignal::Conclude
	OverseerConcluded,
}

impl From<polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>> for Action {
	fn from(
		res: polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>,
	) -> Self {
		match res {
			Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(active_leaves))) => {
				Action::ActiveLeaves(active_leaves)
			}
			Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number))) => {
				Action::BlockFinalized(number)
			}
			Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => {
				Action::Abort(AbortReason::OverseerConcluded)
			}
			Ok(FromOverseer::Communication { msg }) => match msg {
				NetworkBridgeMessage::ReportPeer(peer, rep) => Action::ReportPeer(peer, rep),
				NetworkBridgeMessage::SendValidationMessage(peers, msg) => {
					Action::SendValidationMessages(vec![(peers, msg)])
				}
				NetworkBridgeMessage::SendCollationMessage(peers, msg) => {
					Action::SendCollationMessages(vec![(peers, msg)])
				}
				NetworkBridgeMessage::SendRequests(reqs) => Action::SendRequests(reqs),
				NetworkBridgeMessage::SendValidationMessages(msgs) => {
					Action::SendValidationMessages(msgs)
				}
				NetworkBridgeMessage::SendCollationMessages(msgs) => {
					Action::SendCollationMessages(msgs)
				}
				NetworkBridgeMessage::ConnectToValidators {
					validator_ids,
					peer_set,
					connected,
				} => Action::ConnectToValidators {
					validator_ids,
					peer_set,
					connected,
				},
			},
			Err(e) => Action::Abort(AbortReason::SubsystemError(e)),
		}
	}
}

impl From<Option<NetworkEvent>> for Action {
	fn from(event: Option<NetworkEvent>) -> Action {
		match event {
			None => Action::Abort(AbortReason::EventStreamConcluded),
			Some(NetworkEvent::Dht(_))
			| Some(NetworkEvent::SyncConnected { .. })
			| Some(NetworkEvent::SyncDisconnected { .. }) => Action::Nop,
			Some(NetworkEvent::NotificationStreamOpened {
				remote,
				protocol,
				role,
			}) => {
				let role = role.into();
				PeerSet::try_from_protocol_name(&protocol).map_or(Action::Nop, |peer_set| {
					Action::PeerConnected(peer_set, remote, role)
				})
			}
			Some(NetworkEvent::NotificationStreamClosed { remote, protocol }) => {
				PeerSet::try_from_protocol_name(&protocol).map_or(Action::Nop, |peer_set| {
					Action::PeerDisconnected(peer_set, remote)
				})
			}
			Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
				let v_messages: Result<Vec<_>, _> = messages
					.iter()
					.filter(|(protocol, _)| {
						protocol == &PeerSet::Validation.into_protocol_name()
					})
					.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
					.collect();

				let v_messages = match v_messages {
					Err(_) => return Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
					Ok(v) => v,
				};

				let c_messages: Result<Vec<_>, _> = messages
					.iter()
					.filter(|(protocol, _)| {
						protocol == &PeerSet::Collation.into_protocol_name()
					})
					.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
					.collect();

				match c_messages {
					Err(_) => Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
					Ok(c_messages) => {
						if v_messages.is_empty() && c_messages.is_empty() {
							Action::Nop
						} else {
							Action::PeerMessages(remote, v_messages, c_messages)
						}
					}
				}
			}
		}
	}
}

impl From<Option<Result<AllMessages, RequestMultiplexError>>> for Action {
	fn from(event: Option<Result<AllMessages, RequestMultiplexError>>) -> Self {
		match event {
			None => Action::Abort(AbortReason::RequestStreamConcluded),
			Some(Err(err)) => Action::ReportPeer(err.peer, MALFORMED_MESSAGE_COST),
			Some(Ok(msg)) => Action::SendMessage(msg),
		}
	}
}
