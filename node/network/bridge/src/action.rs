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

use bytes::Bytes;
use parity_scale_codec::{Decode, codec};
use polkadot_node_network_protocol::{
	peer_set::PeerSet, v1 as protocol_v1, PeerId, ReputationChange, message::ProtocolMessage,
};
use polkadot_primitives::v1::{AuthorityDiscoveryId, BlockNumber};
use polkadot_subsystem::messages::NetworkBridgeMessage;
use polkadot_subsystem::{ActiveLeavesUpdate, FromOverseer, OverseerSignal};
use sc_network::Event as NetworkEvent;

use polkadot_node_network_protocol::ObservedRole;

use super::{WireMessage, LOG_TARGET, MALFORMED_MESSAGE_COST};

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

	/// Ask network to connect to validators.
	ConnectToValidators {
		validator_ids: Vec<AuthorityDiscoveryId>,
		connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	},

	/// Report a peer to the network implementation (decreasing/increasing its reputation).
	ReportPeer(PeerId, ReputationChange),

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
	///
	/// We need `PeerSet` here because of the `ViewUpdate` constructor of `WireMessage`. We could
	/// also just split `WireMessage` up here already, but considering that we might remove it,
	/// let's not bother for now.
	PeerMessages(Vec<(PeerSet, WireMessage<ProtocolMessage>)>),

	Abort,
	Nop,
}

impl From<polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>> for Action {
	#[tracing::instrument(level = "trace", fields(subsystem = LOG_TARGET))]
	fn from(res: polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>) -> Self {
		match res {
			Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(active_leaves))) => {
				Action::ActiveLeaves(active_leaves)
			}
			Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number))) => {
				Action::BlockFinalized(number)
			}
			Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => Action::Abort,
			Ok(FromOverseer::Communication { msg }) => match msg {
				NetworkBridgeMessage::ReportPeer(peer, rep) => Action::ReportPeer(peer, rep),
				NetworkBridgeMessage::SendValidationMessage(peers, msg) => {
					Action::SendValidationMessages(vec![(peers, msg)])
				}
				NetworkBridgeMessage::SendCollationMessage(peers, msg) => {
					Action::SendCollationMessages(vec![(peers, msg)])
				}
				NetworkBridgeMessage::SendValidationMessages(msgs) => {
					Action::SendValidationMessages(msgs)
				}
				NetworkBridgeMessage::SendCollationMessages(msgs) => {
					Action::SendCollationMessages(msgs)
				}
				NetworkBridgeMessage::ConnectToValidators {
					validator_ids,
					connected,
				} => Action::ConnectToValidators {
					validator_ids,
					connected,
				},
			},
			Err(e) => {
				tracing::warn!(target: LOG_TARGET, err = ?e, "Shutting down Network Bridge due to error");
				Action::Abort
			}
		}
	}
}

impl From<Option<NetworkEvent>> for Action {
	#[tracing::instrument(level = "trace", fields(subsystem = LOG_TARGET))]
	fn from(event: Option<NetworkEvent>) -> Action {
		match event {
			None => {
				tracing::info!(
					target: LOG_TARGET,
					"Shutting down Network Bridge: underlying event stream concluded"
				);
				Action::Abort
			}
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
				let decoded: Result<Vec<_>, _> = messages
					.iter()
					// We simply ignore unknown protocols:
					.filter_map(|(protocol, msg)|
						 PeerSet::try_from_protocol_name(protocol)
						 .map(|p| Ok((p, decode_wire_message(p, msg)?)))
					)
					.collect();

				match decoded {
					Err(_) => Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
					Ok(v) => {
						if v.is_empty() {
							Action::Nop
						}
						else {
							Action::PeerMessages(v)
						}
					}
				}
			}
		}
	}
}

// Once we removed `WireMessage` we can decode messages by means of `PeerSet::decode_message`.
fn decode_wire_message(peer_set: PeerSet, bytes: &mut Bytes) -> Result<WireMessage<ProtocolMessage>, codec::Error> {

    fn traverse<M, N>(wm: WireMessage<M>, f: impl FnOnce(M) -> N) -> WireMessage<N> {
        match wm {
            WireMessage::ProtocolMessage(m) => WireMessage::ProtocolMessage(f(m)),
            WireMessage::ViewUpdate(v) => WireMessage::ViewUpdate(v),
        }
    }

    let r = match peer_set {
        PeerSet::Validation =>
            traverse(
				Decode::decode(&mut bytes.as_ref())?,
				ProtocolMessage::Validation
			),
        PeerSet::Collation =>
            traverse(Decode::decode(&mut bytes.as_ref())?, ProtocolMessage::Collation),
        PeerSet::AvailabilityDistribution =>
            traverse(
                Decode::decode(&mut bytes.as_ref())?,
                ProtocolMessage::AvailabilityDistribution
                ),
    };
    Ok(r)
}
