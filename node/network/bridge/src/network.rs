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

use std::{borrow::Cow, collections::HashSet, sync::Arc};

use async_trait::async_trait;
use futures::{prelude::*, stream::BoxStream};

use parity_scale_codec::Encode;

use sc_network::{
	config::parse_addr, multiaddr::Multiaddr, Event as NetworkEvent, IfDisconnected,
	NetworkService, OutboundFailure, RequestFailure,
};

use polkadot_node_network_protocol::{
	peer_set::PeerSet,
	request_response::{OutgoingRequest, Recipient, Requests},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::v1::{AuthorityDiscoveryId, Block, Hash};

use crate::validator_discovery::AuthorityDiscovery;

use super::LOG_TARGET;

/// Send a message to the network.
///
/// This function is only used internally by the network-bridge, which is responsible to only send
/// messages that are compatible with the passed peer set, as that is currently not enforced by
/// this function. These are messages of type `WireMessage` parameterized on the matching type.
pub(crate) fn send_message<M>(
	net: &mut impl Network,
	mut peers: Vec<PeerId>,
	peer_set: PeerSet,
	message: M,
	metrics: &super::Metrics,
) where
	M: Encode + Clone,
{
	let message = {
		let encoded = message.encode();
		metrics.on_notification_sent(peer_set, encoded.len(), peers.len());
		encoded
	};

	// optimization: avoid cloning the message for the last peer in the
	// list. The message payload can be quite large. If the underlying
	// network used `Bytes` this would not be necessary.
	let last_peer = peers.pop();
	peers.into_iter().for_each(|peer| {
		net.write_notification(peer, peer_set, message.clone());
	});
	if let Some(peer) = last_peer {
		net.write_notification(peer, peer_set, message);
	}
}

/// An abstraction over networking for the purposes of this subsystem.
#[async_trait]
pub trait Network: Clone + Send + 'static {
	/// Get a stream of all events occurring on the network. This may include events unrelated
	/// to the Polkadot protocol - the user of this function should filter only for events related
	/// to the [`VALIDATION_PROTOCOL_NAME`](VALIDATION_PROTOCOL_NAME)
	/// or [`COLLATION_PROTOCOL_NAME`](COLLATION_PROTOCOL_NAME)
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent>;

	/// Ask the network to keep a substream open with these nodes and not disconnect from them
	/// until removed from the protocol's peer set.
	/// Note that `out_peers` setting has no effect on this.
	async fn set_reserved_peers(
		&mut self,
		protocol: Cow<'static, str>,
		multiaddresses: HashSet<Multiaddr>,
	) -> Result<(), String>;

	/// Removes the peers for the protocol's peer set (both reserved and non-reserved).
	async fn remove_from_peers_set(&mut self, protocol: Cow<'static, str>, peers: Vec<PeerId>);

	/// Send a request to a remote peer.
	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		authority_discovery: &mut AD,
		req: Requests,
		if_disconnected: IfDisconnected,
	);

	/// Report a given peer as either beneficial (+) or costly (-) according to the given scalar.
	fn report_peer(&self, who: PeerId, cost_benefit: Rep);

	/// Disconnect a given peer from the peer set specified without harming reputation.
	fn disconnect_peer(&self, who: PeerId, peer_set: PeerSet);

	/// Write a notification to a peer on the given peer-set's protocol.
	fn write_notification(&self, who: PeerId, peer_set: PeerSet, message: Vec<u8>);
}

#[async_trait]
impl Network for Arc<NetworkService<Block, Hash>> {
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
		NetworkService::event_stream(self, "polkadot-network-bridge").boxed()
	}

	async fn set_reserved_peers(
		&mut self,
		protocol: Cow<'static, str>,
		multiaddresses: HashSet<Multiaddr>,
	) -> Result<(), String> {
		sc_network::NetworkService::set_reserved_peers(&**self, protocol, multiaddresses)
	}

	async fn remove_from_peers_set(&mut self, protocol: Cow<'static, str>, peers: Vec<PeerId>) {
		sc_network::NetworkService::remove_peers_from_reserved_set(&**self, protocol, peers);
	}

	fn report_peer(&self, who: PeerId, cost_benefit: Rep) {
		sc_network::NetworkService::report_peer(&**self, who, cost_benefit.into_base_rep());
	}

	fn disconnect_peer(&self, who: PeerId, peer_set: PeerSet) {
		sc_network::NetworkService::disconnect_peer(&**self, who, peer_set.into_protocol_name());
	}

	fn write_notification(&self, who: PeerId, peer_set: PeerSet, message: Vec<u8>) {
		sc_network::NetworkService::write_notification(
			&**self,
			who,
			peer_set.into_protocol_name(),
			message,
		);
	}

	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		authority_discovery: &mut AD,
		req: Requests,
		if_disconnected: IfDisconnected,
	) {
		let (protocol, OutgoingRequest { peer, payload, pending_response }) = req.encode_request();

		let peer_id = match peer {
			Recipient::Peer(peer_id) => Some(peer_id),
			Recipient::Authority(authority) => {
				let mut found_peer_id = None;
				// Note: `get_addresses_by_authority_id` searched in a cache, and it thus expected
				// to be very quick.
				for addr in authority_discovery
					.get_addresses_by_authority_id(authority)
					.await
					.into_iter()
					.flat_map(|list| list.into_iter())
				{
					let (peer_id, addr) = match parse_addr(addr) {
						Ok(v) => v,
						Err(_) => continue,
					};
					NetworkService::add_known_address(&*self, peer_id.clone(), addr);
					found_peer_id = Some(peer_id);
				}
				found_peer_id
			},
		};

		let peer_id = match peer_id {
			None => {
				tracing::debug!(target: LOG_TARGET, "Discovering authority failed");
				match pending_response
					.send(Err(RequestFailure::Network(OutboundFailure::DialFailure)))
				{
					Err(_) => tracing::debug!(
						target: LOG_TARGET,
						"Sending failed request response failed."
					),
					Ok(_) => {},
				}
				return
			},
			Some(peer_id) => peer_id,
		};

		NetworkService::start_request(
			&*self,
			peer_id,
			protocol.into_protocol_name(),
			payload,
			pending_response,
			if_disconnected,
		);
	}
}

/// We assume one `peer_id` per `authority_id`.
pub async fn get_peer_id_by_authority_id<AD: AuthorityDiscovery>(
	authority_discovery: &mut AD,
	authority: AuthorityDiscoveryId,
) -> Option<PeerId> {
	// Note: `get_addresses_by_authority_id` searched in a cache, and it thus expected
	// to be very quick.
	authority_discovery
		.get_addresses_by_authority_id(authority)
		.await
		.into_iter()
		.flat_map(|list| list.into_iter())
		.find_map(|addr| parse_addr(addr).ok().map(|(p, _)| p))
}
