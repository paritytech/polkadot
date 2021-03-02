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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::stream::BoxStream;

use parity_scale_codec::Encode;

use sc_network::Event as NetworkEvent;
use sc_network::{IfDisconnected, NetworkService, OutboundFailure, RequestFailure};

use polkadot_node_network_protocol::{
	peer_set::PeerSet,
	request_response::{OutgoingRequest, Requests},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::v1::{Block, Hash};
use polkadot_subsystem::{SubsystemError, SubsystemResult};

use crate::validator_discovery::{peer_id_from_multiaddr, AuthorityDiscovery};

use super::LOG_TARGET;

/// Send a message to the network.
///
/// This function is only used internally by the network-bridge, which is responsible to only send
/// messages that are compatible with the passed peer set, as that is currently not enforced by
/// this function. These are messages of type `WireMessage` parameterized on the matching type.
pub(crate) async fn send_message<M, I>(
	net: &mut impl Network,
	peers: I,
	peer_set: PeerSet,
	message: M,
) -> SubsystemResult<()>
where
	M: Encode + Clone,
	I: IntoIterator<Item = PeerId>,
	I::IntoIter: ExactSizeIterator,
{
	let mut message_producer = stream::iter({
		let peers = peers.into_iter();
		let n_peers = peers.len();
		let mut message = Some(message.encode());

		peers.enumerate().map(move |(i, peer)| {
			// optimization: avoid cloning the message for the last peer in the
			// list. The message payload can be quite large. If the underlying
			// network used `Bytes` this would not be necessary.
			let message = if i == n_peers - 1 {
				message
					.take()
					.expect("Only taken in last iteration of loop, never afterwards; qed")
			} else {
				message
					.as_ref()
					.expect("Only taken in last iteration of loop, we are not there yet; qed")
					.clone()
			};

			Ok(NetworkAction::WriteNotification(peer, peer_set, message))
		})
	});

	net.action_sink().send_all(&mut message_producer).await
}

/// An action to be carried out by the network.
///
/// This type is used for implementing `Sink` in order to cummunicate asynchronously with the
/// underlying network implementation in the `Network` trait.
#[derive(Debug, PartialEq)]
pub enum NetworkAction {
	/// Note a change in reputation for a peer.
	ReputationChange(PeerId, Rep),
	/// Write a notification to a given peer on the given peer-set.
	WriteNotification(PeerId, PeerSet, Vec<u8>),
}

/// An abstraction over networking for the purposes of this subsystem.
#[async_trait]
pub trait Network: Send + 'static {
	/// Get a stream of all events occurring on the network. This may include events unrelated
	/// to the Polkadot protocol - the user of this function should filter only for events related
	/// to the [`VALIDATION_PROTOCOL_NAME`](VALIDATION_PROTOCOL_NAME)
	/// or [`COLLATION_PROTOCOL_NAME`](COLLATION_PROTOCOL_NAME)
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent>;

	/// Get access to an underlying sink for all network actions.
	fn action_sink<'a>(
		&'a mut self,
	) -> Pin<Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>>;

	/// Send a request to a remote peer.
	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		authority_discovery: &mut AD,
		req: Requests,
	);

	/// Report a given peer as either beneficial (+) or costly (-) according to the given scalar.
	fn report_peer(
		&mut self,
		who: PeerId,
		cost_benefit: Rep,
	) -> BoxFuture<SubsystemResult<()>> {
		async move {
			self.action_sink()
				.send(NetworkAction::ReputationChange(who, cost_benefit))
				.await
		}
		.boxed()
	}

	/// Write a notification to a peer on the given peer-set's protocol.
	fn write_notification(
		&mut self,
		who: PeerId,
		peer_set: PeerSet,
		message: Vec<u8>,
	) -> BoxFuture<SubsystemResult<()>> {
		async move {
			self.action_sink()
				.send(NetworkAction::WriteNotification(who, peer_set, message))
				.await
		}
		.boxed()
	}
}

#[async_trait]
impl Network for Arc<NetworkService<Block, Hash>> {
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
		NetworkService::event_stream(self, "polkadot-network-bridge").boxed()
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn action_sink<'a>(
		&'a mut self,
	) -> Pin<Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>> {
		use futures::task::{Context, Poll};

		// wrapper around a NetworkService to make it act like a sink.
		struct ActionSink<'b>(&'b NetworkService<Block, Hash>);

		impl<'b> Sink<NetworkAction> for ActionSink<'b> {
			type Error = SubsystemError;

			fn poll_ready(self: Pin<&mut Self>, _: &mut Context) -> Poll<SubsystemResult<()>> {
				Poll::Ready(Ok(()))
			}

			fn start_send(self: Pin<&mut Self>, action: NetworkAction) -> SubsystemResult<()> {
				match action {
					NetworkAction::ReputationChange(peer, cost_benefit) => {
						tracing::debug!(
							target: LOG_TARGET,
							"Changing reputation: {:?} for {}",
							cost_benefit,
							peer
						);
						self.0.report_peer(peer, cost_benefit.into_base_rep())
					}
					NetworkAction::WriteNotification(peer, peer_set, message) => self
						.0
						.write_notification(peer, peer_set.into_protocol_name(), message),
				}

				Ok(())
			}

			fn poll_flush(self: Pin<&mut Self>, _: &mut Context) -> Poll<SubsystemResult<()>> {
				Poll::Ready(Ok(()))
			}

			fn poll_close(self: Pin<&mut Self>, _: &mut Context) -> Poll<SubsystemResult<()>> {
				Poll::Ready(Ok(()))
			}
		}

		Box::pin(ActionSink(&**self))
	}

	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		authority_discovery: &mut AD,
		req: Requests,
	) {
		let (
			protocol,
			OutgoingRequest {
				peer,
				payload,
				pending_response,
			},
		) = req.encode_request();

		let peer_id = authority_discovery
			.get_addresses_by_authority_id(peer)
			.await
			.and_then(|addrs| {
				addrs
					.into_iter()
					.find_map(|addr| peer_id_from_multiaddr(&addr))
			});

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
					Ok(_) => {}
				}
				return;
			}
			Some(peer_id) => peer_id,
		};

		NetworkService::start_request(
			&*self,
			peer_id,
			protocol.into_protocol_name(),
			payload,
			pending_response,
			IfDisconnected::TryConnect,
		);
	}
}
