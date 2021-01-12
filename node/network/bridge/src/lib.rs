// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The Network Bridge Subsystem - protocol multiplexer for Polkadot.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]


use parity_scale_codec::{Encode, Decode};
use futures::prelude::*;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::channel::mpsc;

use sc_network::Event as NetworkEvent;

use polkadot_subsystem::{
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemError,
	SubsystemResult, JaegerSpan,
};
use polkadot_subsystem::messages::{
	NetworkBridgeMessage, AllMessages, AvailabilityDistributionMessage,
	BitfieldDistributionMessage, PoVDistributionMessage, StatementDistributionMessage,
	CollatorProtocolMessage,
};
use polkadot_primitives::v1::{AuthorityDiscoveryId, Block, Hash, BlockNumber};
use polkadot_node_network_protocol::{
	ObservedRole, ReputationChange, PeerId, PeerSet, View, NetworkBridgeEvent, v1 as protocol_v1, OurView,
};

use std::collections::{HashMap, hash_map};
use std::iter::ExactSizeIterator;
use std::pin::Pin;
use std::sync::Arc;

mod validator_discovery;

/// The maximum amount of heads a peer is allowed to have in their view at any time.
///
/// We use the same limit to compute the view sent to peers locally.
const MAX_VIEW_HEADS: usize = 5;

/// The protocol name for the validation peer-set.
pub const VALIDATION_PROTOCOL_NAME: &'static str = "/polkadot/validation/1";
/// The protocol name for the collation peer-set.
pub const COLLATION_PROTOCOL_NAME: &'static str = "/polkadot/collation/1";

const MALFORMED_MESSAGE_COST: ReputationChange = ReputationChange::new(-500, "Malformed Network-bridge message");
const UNCONNECTED_PEERSET_COST: ReputationChange = ReputationChange::new(-50, "Message sent to un-connected peer-set");
const MALFORMED_VIEW_COST: ReputationChange = ReputationChange::new(-500, "Malformed view");
const EMPTY_VIEW_COST: ReputationChange = ReputationChange::new(-500, "Peer sent us an empty view");

// network bridge log target
const LOG_TARGET: &'static str = "network_bridge";

/// Messages received on the network.
#[derive(Debug, Encode, Decode, Clone)]
pub enum WireMessage<M> {
	/// A message from a peer on a specific protocol.
	#[codec(index = "1")]
	ProtocolMessage(M),
	/// A view update from a peer.
	#[codec(index = "2")]
	ViewUpdate(View),
}

/// Information about the extra peers set. Should be used during network configuration
/// to register the protocol with the network service.
pub fn peers_sets_info() -> Vec<sc_network::config::NonDefaultSetConfig> {
	vec![
		sc_network::config::NonDefaultSetConfig {
			notifications_protocol: VALIDATION_PROTOCOL_NAME.into(),
			set_config: sc_network::config::SetConfig {
				in_peers: 25,
				out_peers: 0,
				reserved_nodes: Vec::new(),
				non_reserved_mode: sc_network::config::NonReservedPeerMode::Accept,
			},
		},
		sc_network::config::NonDefaultSetConfig {
			notifications_protocol: COLLATION_PROTOCOL_NAME.into(),
			set_config: sc_network::config::SetConfig {
				in_peers: 25,
				out_peers: 0,
				reserved_nodes: Vec::new(),
				non_reserved_mode: sc_network::config::NonReservedPeerMode::Accept,
			},
		}
	]
}

/// An action to be carried out by the network.
#[derive(Debug, PartialEq)]
pub enum NetworkAction {
	/// Note a change in reputation for a peer.
	ReputationChange(PeerId, ReputationChange),
	/// Write a notification to a given peer on the given peer-set.
	WriteNotification(PeerId, PeerSet, Vec<u8>),
}

/// An abstraction over networking for the purposes of this subsystem.
pub trait Network: Send + 'static {
	/// Get a stream of all events occurring on the network. This may include events unrelated
	/// to the Polkadot protocol - the user of this function should filter only for events related
	/// to the [`VALIDATION_PROTOCOL_NAME`](VALIDATION_PROTOCOL_NAME)
	/// or [`COLLATION_PROTOCOL_NAME`](COLLATION_PROTOCOL_NAME)
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent>;

	/// Get access to an underlying sink for all network actions.
	fn action_sink<'a>(&'a mut self) -> Pin<
		Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>
	>;

	/// Report a given peer as either beneficial (+) or costly (-) according to the given scalar.
	fn report_peer(&mut self, who: PeerId, cost_benefit: ReputationChange)
		-> BoxFuture<SubsystemResult<()>>
	{
		async move {
			self.action_sink().send(NetworkAction::ReputationChange(who, cost_benefit)).await
		}.boxed()
	}

	/// Write a notification to a peer on the given peer-set's protocol.
	fn write_notification(&mut self, who: PeerId, peer_set: PeerSet, message: Vec<u8>)
		-> BoxFuture<SubsystemResult<()>>
	{
		async move {
			self.action_sink().send(NetworkAction::WriteNotification(who, peer_set, message)).await
		}.boxed()
	}
}

impl Network for Arc<sc_network::NetworkService<Block, Hash>> {
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
		sc_network::NetworkService::event_stream(self, "polkadot-network-bridge").boxed()
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn action_sink<'a>(&'a mut self)
		-> Pin<Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>>
	{
		use futures::task::{Poll, Context};

		// wrapper around a NetworkService to make it act like a sink.
		struct ActionSink<'b>(&'b sc_network::NetworkService<Block, Hash>);

		impl<'b> Sink<NetworkAction> for ActionSink<'b> {
			type Error = SubsystemError;

			fn poll_ready(self: Pin<&mut Self>, _: &mut Context) -> Poll<SubsystemResult<()>> {
				Poll::Ready(Ok(()))
			}

			fn start_send(self: Pin<&mut Self>, action: NetworkAction) -> SubsystemResult<()> {
				match action {
					NetworkAction::ReputationChange(peer, cost_benefit) => {
						tracing::debug!(target: LOG_TARGET, "Changing reputation: {:?} for {}", cost_benefit, peer);
						self.0.report_peer(
							peer,
							cost_benefit,
						)
					}
					NetworkAction::WriteNotification(peer, peer_set, message) => {
						match peer_set {
							PeerSet::Validation => self.0.write_notification(
								peer,
								VALIDATION_PROTOCOL_NAME.into(),
								message,
							),
							PeerSet::Collation => self.0.write_notification(
								peer,
								COLLATION_PROTOCOL_NAME.into(),
								message,
							),
						}
					}
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
}

/// The network bridge subsystem.
pub struct NetworkBridge<N, AD> {
	network_service: N,
	authority_discovery_service: AD,
}

impl<N, AD> NetworkBridge<N, AD> {
	/// Create a new network bridge subsystem with underlying network service and authority discovery service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`peers_sets_info`](peers_sets_info).
	pub fn new(network_service: N, authority_discovery_service: AD) -> Self {
		NetworkBridge {
			network_service,
			authority_discovery_service,
		}
	}
}

impl<Net, AD, Context> Subsystem<Context> for NetworkBridge<Net, AD>
	where
		Net: Network + validator_discovery::Network,
		AD: validator_discovery::AuthorityDiscovery,
		Context: SubsystemContext<Message=NetworkBridgeMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		let Self { network_service, authority_discovery_service } = self;
		let future = run_network(
				network_service,
				authority_discovery_service,
				ctx,
			)
			.map_err(|e| {
				SubsystemError::with_origin("network-bridge", e)
			})
			.boxed();
		SpawnedSubsystem {
			name: "network-bridge-subsystem",
			future,
		}
	}
}

struct PeerData {
	/// Latest view sent by the peer.
	view: View,
}

#[derive(Debug)]
enum Action {
	SendValidationMessages(Vec<(Vec<PeerId>, protocol_v1::ValidationProtocol)>),
	SendCollationMessages(Vec<(Vec<PeerId>, protocol_v1::CollationProtocol)>),
	ConnectToValidators {
		validator_ids: Vec<AuthorityDiscoveryId>,
		connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	},
	ReportPeer(PeerId, ReputationChange),

	ActiveLeaves(ActiveLeavesUpdate),
	BlockFinalized(BlockNumber),

	PeerConnected(PeerSet, PeerId, ObservedRole),
	PeerDisconnected(PeerSet, PeerId),
	PeerMessages(
		PeerId,
		Vec<WireMessage<protocol_v1::ValidationProtocol>>,
		Vec<WireMessage<protocol_v1::CollationProtocol>>,
	),

	Abort,
	Nop,
}

#[tracing::instrument(level = "trace", fields(subsystem = LOG_TARGET))]
fn action_from_overseer_message(
	res: polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>,
) -> Action {
	match res {
		Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(active_leaves)))
			=> Action::ActiveLeaves(active_leaves),
		Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number)))
			=> Action::BlockFinalized(number),
		Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => Action::Abort,
		Ok(FromOverseer::Communication { msg }) => match msg {
			NetworkBridgeMessage::ReportPeer(peer, rep) => Action::ReportPeer(peer, rep),
			NetworkBridgeMessage::SendValidationMessage(peers, msg)
				=> Action::SendValidationMessages(vec![(peers, msg)]),
			NetworkBridgeMessage::SendCollationMessage(peers, msg)
				=> Action::SendCollationMessages(vec![(peers, msg)]),
			NetworkBridgeMessage::SendValidationMessages(msgs)
				=> Action::SendValidationMessages(msgs),
			NetworkBridgeMessage::SendCollationMessages(msgs)
				=> Action::SendCollationMessages(msgs),
			NetworkBridgeMessage::ConnectToValidators { validator_ids, connected }
				=> Action::ConnectToValidators { validator_ids, connected },
		},
		Err(e) => {
			tracing::warn!(target: LOG_TARGET, err = ?e, "Shutting down Network Bridge due to error");
			Action::Abort
		}
	}
}

#[tracing::instrument(level = "trace", fields(subsystem = LOG_TARGET))]
fn action_from_network_message(event: Option<NetworkEvent>) -> Action {
	match event {
		None => {
			tracing::info!(target: LOG_TARGET, "Shutting down Network Bridge: underlying event stream concluded");
			Action::Abort
		}
		Some(NetworkEvent::Dht(_)) |
		Some(NetworkEvent::SyncConnected { .. }) |
		Some(NetworkEvent::SyncDisconnected { .. }) => Action::Nop,
		Some(NetworkEvent::NotificationStreamOpened { remote, protocol, role }) => {
			let role = role.into();
			match protocol {
				x if x == VALIDATION_PROTOCOL_NAME
					=> Action::PeerConnected(PeerSet::Validation, remote, role),
				x if x == COLLATION_PROTOCOL_NAME
					=> Action::PeerConnected(PeerSet::Collation, remote, role),
				_ => Action::Nop,
			}
		}
		Some(NetworkEvent::NotificationStreamClosed { remote, protocol }) => {
			match protocol {
				x if x == VALIDATION_PROTOCOL_NAME
					=> Action::PeerDisconnected(PeerSet::Validation, remote),
				x if x == COLLATION_PROTOCOL_NAME
					=> Action::PeerDisconnected(PeerSet::Collation, remote),
				_ => Action::Nop,
			}
		}
		Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
			let v_messages: Result<Vec<_>, _> = messages.iter()
				.filter(|(protocol, _)| protocol == &VALIDATION_PROTOCOL_NAME)
				.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
				.collect();

			let v_messages = match v_messages {
				Err(_) => return Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
				Ok(v) => v,
			};

			let c_messages: Result<Vec<_>, _> = messages.iter()
				.filter(|(protocol, _)| protocol == &COLLATION_PROTOCOL_NAME)
				.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
				.collect();

			match c_messages {
				Err(_) => Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
				Ok(c_messages) => if v_messages.is_empty() && c_messages.is_empty() {
					Action::Nop
				} else {
					Action::PeerMessages(remote, v_messages, c_messages)
				},
			}
		}
	}
}

fn construct_view(live_heads: impl DoubleEndedIterator<Item = Hash>, finalized_number: BlockNumber) -> View {
	View {
		heads: live_heads.rev().take(MAX_VIEW_HEADS).collect(),
		finalized_number
	}
}

#[tracing::instrument(level = "trace", skip(net, ctx, validation_peers, collation_peers), fields(subsystem = LOG_TARGET))]
async fn update_our_view(
	net: &mut impl Network,
	ctx: &mut impl SubsystemContext<Message = NetworkBridgeMessage>,
	live_heads: &[(Hash, Arc<JaegerSpan>)],
	local_view: &mut View,
	finalized_number: BlockNumber,
	validation_peers: &HashMap<PeerId, PeerData>,
	collation_peers: &HashMap<PeerId, PeerData>,
) -> SubsystemResult<()> {
	let new_view = construct_view(live_heads.iter().map(|v| v.0), finalized_number);

	// We only want to send a view update when the heads changed, not when only the finalized block changed.
	if local_view.heads == new_view.heads {
		return Ok(())
	}

	*local_view = new_view.clone();

	send_validation_message(
		net,
		validation_peers.keys().cloned(),
		WireMessage::ViewUpdate(new_view.clone()),
	).await?;

	send_collation_message(
		net,
		collation_peers.keys().cloned(),
		WireMessage::ViewUpdate(new_view),
	).await?;

	let our_view = OurView::new(live_heads.iter().cloned(), finalized_number);

	dispatch_validation_event_to_all(NetworkBridgeEvent::OurViewChange(our_view.clone()), ctx).await;

	dispatch_collation_event_to_all(NetworkBridgeEvent::OurViewChange(our_view), ctx).await;

	Ok(())
}

// Handle messages on a specific peer-set. The peer is expected to be connected on that
// peer-set.
#[tracing::instrument(level = "trace", skip(peers, messages, net), fields(subsystem = LOG_TARGET))]
async fn handle_peer_messages<M>(
	peer: PeerId,
	peers: &mut HashMap<PeerId, PeerData>,
	messages: Vec<WireMessage<M>>,
	net: &mut impl Network,
) -> SubsystemResult<Vec<NetworkBridgeEvent<M>>> {
	let peer_data = match peers.get_mut(&peer) {
		None => {
			net.report_peer(peer, UNCONNECTED_PEERSET_COST).await?;

			return Ok(Vec::new());
		},
		Some(d) => d,
	};

	let mut outgoing_messages = Vec::with_capacity(messages.len());
	for message in messages {
		outgoing_messages.push(match message {
			WireMessage::ViewUpdate(new_view) => {
				if new_view.heads.len() > MAX_VIEW_HEADS {
					net.report_peer(
						peer.clone(),
						MALFORMED_VIEW_COST,
					).await?;

					continue
				} else if new_view.heads.is_empty() {
					net.report_peer(
						peer.clone(),
						EMPTY_VIEW_COST,
					).await?;

					continue
				} else if new_view == peer_data.view {
					continue
				} else {
					peer_data.view = new_view;

					NetworkBridgeEvent::PeerViewChange(
						peer.clone(),
						peer_data.view.clone(),
					)
				}
			}
			WireMessage::ProtocolMessage(message) => {
				NetworkBridgeEvent::PeerMessage(peer.clone(), message)
			}
		})
	}

	Ok(outgoing_messages)
}

#[tracing::instrument(level = "trace", skip(net, peers), fields(subsystem = LOG_TARGET))]
async fn send_validation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::ValidationProtocol>,
) -> SubsystemResult<()>
	where
		I: IntoIterator<Item=PeerId>,
		I::IntoIter: ExactSizeIterator,
{
	send_message(net, peers, PeerSet::Validation, message).await
}

#[tracing::instrument(level = "trace", skip(net, peers), fields(subsystem = LOG_TARGET))]
async fn send_collation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::CollationProtocol>,
) -> SubsystemResult<()>
	where
	I: IntoIterator<Item=PeerId>,
	I::IntoIter: ExactSizeIterator,
{
	send_message(net, peers, PeerSet::Collation, message).await
}

async fn send_message<M, I>(
	net: &mut impl Network,
	peers: I,
	peer_set: PeerSet,
	message: WireMessage<M>,
) -> SubsystemResult<()>
	where
		M: Encode + Clone,
		I: IntoIterator<Item=PeerId>,
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
				message.take()
					.expect("Only taken in last iteration of loop, never afterwards; qed")
			} else {
				message.as_ref()
					.expect("Only taken in last iteration of loop, we are not there yet; qed")
					.clone()
			};

			Ok(NetworkAction::WriteNotification(peer, peer_set, message))
		})
	});

	net.action_sink().send_all(&mut message_producer).await
}

async fn dispatch_validation_event_to_all(
	event: NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
	ctx: &mut impl SubsystemContext<Message=NetworkBridgeMessage>,
) {
	dispatch_validation_events_to_all(std::iter::once(event), ctx).await
}

async fn dispatch_collation_event_to_all(
	event: NetworkBridgeEvent<protocol_v1::CollationProtocol>,
	ctx: &mut impl SubsystemContext<Message=NetworkBridgeMessage>,
) {
	dispatch_collation_events_to_all(std::iter::once(event), ctx).await
}

#[tracing::instrument(level = "trace", skip(events, ctx), fields(subsystem = LOG_TARGET))]
async fn dispatch_validation_events_to_all<I>(
	events: I,
	ctx: &mut impl SubsystemContext<Message=NetworkBridgeMessage>,
)
	where
		I: IntoIterator<Item = NetworkBridgeEvent<protocol_v1::ValidationProtocol>>,
		I::IntoIter: Send,
{
	let messages_for = |event: NetworkBridgeEvent<protocol_v1::ValidationProtocol>| {
		let a = std::iter::once(event.focus().ok().map(|m| AllMessages::AvailabilityDistribution(
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(m)
		)));

		let b = std::iter::once(event.focus().ok().map(|m| AllMessages::BitfieldDistribution(
			BitfieldDistributionMessage::NetworkBridgeUpdateV1(m)
		)));

		let p = std::iter::once(event.focus().ok().map(|m| AllMessages::PoVDistribution(
			PoVDistributionMessage::NetworkBridgeUpdateV1(m)
		)));

		let s = std::iter::once(event.focus().ok().map(|m| AllMessages::StatementDistribution(
			StatementDistributionMessage::NetworkBridgeUpdateV1(m)
		)));

		a.chain(b).chain(p).chain(s).filter_map(|x| x)
	};

	ctx.send_messages(events.into_iter().flat_map(messages_for)).await
}

#[tracing::instrument(level = "trace", skip(events, ctx), fields(subsystem = LOG_TARGET))]
async fn dispatch_collation_events_to_all<I>(
	events: I,
	ctx: &mut impl SubsystemContext<Message=NetworkBridgeMessage>,
)
	where
		I: IntoIterator<Item = NetworkBridgeEvent<protocol_v1::CollationProtocol>>,
		I::IntoIter: Send,
{
	let messages_for = |event: NetworkBridgeEvent<protocol_v1::CollationProtocol>| {
		event.focus().ok().map(|m| AllMessages::CollatorProtocol(
			CollatorProtocolMessage::NetworkBridgeUpdateV1(m)
		))
	};

	ctx.send_messages(events.into_iter().flat_map(messages_for)).await
}

#[tracing::instrument(skip(network_service, authority_discovery_service, ctx), fields(subsystem = LOG_TARGET))]
async fn run_network<N, AD>(
	mut network_service: N,
	mut authority_discovery_service: AD,
	mut ctx: impl SubsystemContext<Message=NetworkBridgeMessage>,
) -> SubsystemResult<()>
where
	N: Network + validator_discovery::Network,
	AD: validator_discovery::AuthorityDiscovery,
{
	let mut event_stream = network_service.event_stream().fuse();

	// Most recent heads are at the back.
	let mut live_heads: Vec<(Hash, Arc<JaegerSpan>)> = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut local_view = View::default();
	let mut finalized_number = 0;

	let mut validation_peers: HashMap<PeerId, PeerData> = HashMap::new();
	let mut collation_peers: HashMap<PeerId, PeerData> = HashMap::new();

	let mut validator_discovery = validator_discovery::Service::<N, AD>::new();

	loop {

		let action = {
			let subsystem_next = ctx.recv().fuse();
			let mut net_event_next = event_stream.next().fuse();
			futures::pin_mut!(subsystem_next);

			futures::select! {
				subsystem_msg = subsystem_next => action_from_overseer_message(subsystem_msg),
				net_event = net_event_next => action_from_network_message(net_event),
			}
		};

		match action {
			Action::Nop => {}
			Action::Abort => return Ok(()),

			Action::SendValidationMessages(msgs) => {
				for (peers, msg) in msgs {
					send_message(
							&mut network_service,
							peers,
							PeerSet::Validation,
							WireMessage::ProtocolMessage(msg),
					).await?
				}
			}

			Action::SendCollationMessages(msgs) => {
				for (peers, msg) in msgs {
					send_message(
							&mut network_service,
							peers,
							PeerSet::Collation,
							WireMessage::ProtocolMessage(msg),
					).await?
				}
			}

			Action::ConnectToValidators {
				validator_ids,
				connected,
			} => {
				let (ns, ads) = validator_discovery.on_request(
					validator_ids,
					connected,
					network_service,
					authority_discovery_service,
				).await;
				network_service = ns;
				authority_discovery_service = ads;
			},

			Action::ReportPeer(peer, rep) => network_service.report_peer(peer, rep).await?,

			Action::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated }) => {
				live_heads.extend(activated);
				live_heads.retain(|h| !deactivated.contains(&h.0));

				update_our_view(
					&mut network_service,
					&mut ctx,
					&live_heads,
					&mut local_view,
					finalized_number,
					&validation_peers,
					&collation_peers,
				).await?;
			}

			Action::BlockFinalized(number) => {
				debug_assert!(finalized_number < number);

				// we don't send the view updates here, but delay them until the next `Action::ActiveLeaves`
				// otherwise it might break assumptions of some of the subsystems
				// that we never send the same `ActiveLeavesUpdate`
				// this is fine, we will get `Action::ActiveLeaves` on block finalization anyway
				finalized_number = number;
			},

			Action::PeerConnected(peer_set, peer, role) => {
				let peer_map = match peer_set {
					PeerSet::Validation => &mut validation_peers,
					PeerSet::Collation => &mut collation_peers,
				};

				validator_discovery.on_peer_connected(&peer, &mut authority_discovery_service).await;

				match peer_map.entry(peer.clone()) {
					hash_map::Entry::Occupied(_) => continue,
					hash_map::Entry::Vacant(vacant) => {
						let _ = vacant.insert(PeerData {
							view: View::default(),
						});

						match peer_set {
							PeerSet::Validation => dispatch_validation_events_to_all(
								vec![
									NetworkBridgeEvent::PeerConnected(peer.clone(), role),
									NetworkBridgeEvent::PeerViewChange(
										peer,
										View::default(),
									),
								],
								&mut ctx,
							).await,
							PeerSet::Collation => dispatch_collation_events_to_all(
								vec![
									NetworkBridgeEvent::PeerConnected(peer.clone(), role),
									NetworkBridgeEvent::PeerViewChange(
										peer,
										View::default(),
									),
								],
								&mut ctx,
							).await,
						}
					}
				}
			}
			Action::PeerDisconnected(peer_set, peer) => {
				let peer_map = match peer_set {
					PeerSet::Validation => &mut validation_peers,
					PeerSet::Collation => &mut collation_peers,
				};

				validator_discovery.on_peer_disconnected(&peer);

				if peer_map.remove(&peer).is_some() {
					match peer_set {
						PeerSet::Validation => dispatch_validation_event_to_all(
							NetworkBridgeEvent::PeerDisconnected(peer),
							&mut ctx,
						).await,
						PeerSet::Collation => dispatch_collation_event_to_all(
							NetworkBridgeEvent::PeerDisconnected(peer),
							&mut ctx,
						).await,
					}
				}
			},
			Action::PeerMessages(peer, v_messages, c_messages) => {
				if !v_messages.is_empty() {
					let events = handle_peer_messages(
						peer.clone(),
						&mut validation_peers,
						v_messages,
						&mut network_service,
					).await?;

					dispatch_validation_events_to_all(events, &mut ctx).await;
				}

				if !c_messages.is_empty() {
					let events = handle_peer_messages(
						peer.clone(),
						&mut collation_peers,
						c_messages,
						&mut network_service,
					).await?;

					dispatch_collation_events_to_all(events, &mut ctx).await;
				}
			},
		}
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor;

	use std::borrow::Cow;
	use std::sync::Arc;
	use std::collections::HashSet;
	use async_trait::async_trait;
	use parking_lot::Mutex;
	use assert_matches::assert_matches;

	use polkadot_subsystem::messages::{StatementDistributionMessage, BitfieldDistributionMessage};
	use polkadot_node_subsystem_test_helpers::{
		SingleItemSink, SingleItemStream, TestSubsystemContextHandle,
	};
	use polkadot_node_subsystem_util::metered;
	use polkadot_node_network_protocol::view;
	use sc_network::Multiaddr;
	use sp_keyring::Sr25519Keyring;

	// The subsystem's view of the network - only supports a single call to `event_stream`.
	struct TestNetwork {
		net_events: Arc<Mutex<Option<SingleItemStream<NetworkEvent>>>>,
		action_tx: metered::UnboundedMeteredSender<NetworkAction>,
	}

	struct TestAuthorityDiscovery;

	// The test's view of the network. This receives updates from the subsystem in the form
	// of `NetworkAction`s.
	struct TestNetworkHandle {
		action_rx: metered::UnboundedMeteredReceiver<NetworkAction>,
		net_tx: SingleItemSink<NetworkEvent>,
	}

	fn new_test_network() -> (
		TestNetwork,
		TestNetworkHandle,
		TestAuthorityDiscovery,
	) {
		let (net_tx, net_rx) = polkadot_node_subsystem_test_helpers::single_item_sink();
		let (action_tx, action_rx) = metered::unbounded("test_action");

		(
			TestNetwork {
				net_events: Arc::new(Mutex::new(Some(net_rx))),
				action_tx,
			},
			TestNetworkHandle {
				action_rx,
				net_tx,
			},
			TestAuthorityDiscovery,
		)
	}

	fn peer_set_protocol(peer_set: PeerSet) -> std::borrow::Cow<'static, str> {
		match peer_set {
			PeerSet::Validation => VALIDATION_PROTOCOL_NAME.into(),
			PeerSet::Collation => COLLATION_PROTOCOL_NAME.into(),
		}
	}

	impl Network for TestNetwork {
		fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
			self.net_events.lock()
				.take()
				.expect("Subsystem made more than one call to `event_stream`")
				.boxed()
		}

		fn action_sink<'a>(&'a mut self)
			-> Pin<Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>>
		{
			Box::pin((&mut self.action_tx).sink_map_err(Into::into))
		}
	}

	#[async_trait]
	impl validator_discovery::Network for TestNetwork {
		async fn add_peers_to_reserved_set(&mut self, _protocol: Cow<'static, str>, _: HashSet<Multiaddr>) -> Result<(), String> {
			Ok(())
		}

		async fn remove_peers_from_reserved_set(&mut self, _protocol: Cow<'static, str>, _: HashSet<Multiaddr>) -> Result<(), String> {
			Ok(())
		}
	}

	#[async_trait]
	impl validator_discovery::AuthorityDiscovery for TestAuthorityDiscovery {
		async fn get_addresses_by_authority_id(&mut self, _authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>> {
			None
		}

		async fn get_authority_id_by_peer_id(&mut self, _peer_id: PeerId) -> Option<AuthorityDiscoveryId> {
			None
		}
	}

	impl TestNetworkHandle {
		// Get the next network action.
		async fn next_network_action(&mut self) -> NetworkAction {
			self.action_rx.next().await.expect("subsystem concluded early")
		}

		// Wait for the next N network actions.
		async fn next_network_actions(&mut self, n: usize) -> Vec<NetworkAction> {
			let mut v = Vec::with_capacity(n);
			for _ in 0..n {
				v.push(self.next_network_action().await);
			}

			v
		}

		async fn connect_peer(&mut self, peer: PeerId, peer_set: PeerSet, role: ObservedRole) {
			self.send_network_event(NetworkEvent::NotificationStreamOpened {
				remote: peer,
				protocol: peer_set_protocol(peer_set),
				role: role.into(),
			}).await;
		}

		async fn disconnect_peer(&mut self, peer: PeerId, peer_set: PeerSet) {
			self.send_network_event(NetworkEvent::NotificationStreamClosed {
				remote: peer,
				protocol: peer_set_protocol(peer_set),
			}).await;
		}

		async fn peer_message(&mut self, peer: PeerId, peer_set: PeerSet, message: Vec<u8>) {
			self.send_network_event(NetworkEvent::NotificationsReceived {
				remote: peer,
				messages: vec![(peer_set_protocol(peer_set), message.into())],
			}).await;
		}

		async fn send_network_event(&mut self, event: NetworkEvent) {
			self.net_tx.send(event).await.expect("subsystem concluded early");
		}
	}

	/// Assert that the given actions contain the given `action`.
	fn assert_network_actions_contains(actions: &[NetworkAction], action: &NetworkAction) {
		if !actions.iter().any(|x| x == action) {
			panic!("Could not find `{:?}` in `{:?}`", action, actions);
		}
	}

	struct TestHarness {
		network_handle: TestNetworkHandle,
		virtual_overseer: TestSubsystemContextHandle<NetworkBridgeMessage>,
	}

	fn test_harness<T: Future<Output=()>>(test: impl FnOnce(TestHarness) -> T) {
		let pool = sp_core::testing::TaskExecutor::new();
		let (network, network_handle, discovery) = new_test_network();
		let (context, virtual_overseer) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let network_bridge = run_network(
			network,
			discovery,
			context,
		)
			.map_err(|_| panic!("subsystem execution failed"))
			.map(|_| ());

		let test_fut = test(TestHarness {
			network_handle,
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(network_bridge);

		let _ = executor::block_on(future::select(test_fut, network_bridge));
	}

	async fn assert_sends_validation_event_to_all(
		event: NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
		virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeMessage>,
	) {
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::BitfieldDistribution(
				BitfieldDistributionMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::PoVDistribution(
				PoVDistributionMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		);
	}

	async fn assert_sends_collation_event_to_all(
		event: NetworkBridgeEvent<protocol_v1::CollationProtocol>,
		virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeMessage>,
	) {
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(
				CollatorProtocolMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		)
	}

	#[test]
	fn sends_view_updates_to_peers() {
		test_harness(|test_harness| async move {
			let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			network_handle.connect_peer(
				peer_a.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;
			network_handle.connect_peer(
				peer_b.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;

			let hash_a = Hash::repeat_byte(1);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(JaegerSpan::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(2).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				view![hash_a]
			).encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_a,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_b,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);
		});
	}

	#[test]
	fn do_not_send_view_update_when_only_finalized_block_changed() {
		test_harness(|test_harness| async move {
			let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			network_handle.connect_peer(
				peer_a.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;
			network_handle.connect_peer(
				peer_b.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;

			let hash_a = Hash::repeat_byte(1);

			virtual_overseer.send(FromOverseer::Signal(OverseerSignal::BlockFinalized(Hash::random(), 5))).await;

			// Send some empty active leaves update
			//
			// This should not trigger a view update to our peers.
			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::default()))
			).await;

			// This should trigger the view update to our peers.
			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(JaegerSpan::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(2).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				View { heads: vec![hash_a], finalized_number: 5 }
			).encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_a,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_b,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);
		});
	}

	#[test]
	fn peer_view_updates_sent_via_overseer() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;

			let view = view![Hash::repeat_byte(1)];

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			network_handle.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
					view.clone(),
				).encode(),
			).await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), view),
				&mut virtual_overseer,
			).await;
		});
	}

	#[test]
	fn peer_messages_sent_via_overseer() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(
				peer.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			let pov_distribution_message = protocol_v1::PoVDistributionMessage::Awaiting(
				[0; 32].into(),
				vec![[1; 32].into()],
			);

			let message = protocol_v1::ValidationProtocol::PoVDistribution(
				pov_distribution_message.clone(),
			);

			network_handle.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::ProtocolMessage(message.clone()).encode(),
			).await;

			network_handle.disconnect_peer(peer.clone(), PeerSet::Validation).await;

			// PoV distribution message comes first, and the message is only sent to that subsystem.
			// then a disconnection event arises that is sent to all validation networking subsystems.

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(
					PoVDistributionMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerMessage(p, m)
					)
				) => {
					assert_eq!(p, peer);
					assert_eq!(m, pov_distribution_message);
				}
			);

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerDisconnected(peer),
				&mut virtual_overseer,
			).await;
		});
	}

	#[test]
	fn peer_disconnect_from_just_one_peerset() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			network_handle.disconnect_peer(peer.clone(), PeerSet::Validation).await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerDisconnected(peer.clone()),
				&mut virtual_overseer,
			).await;

			// to show that we're still connected on the collation protocol, send a view update.

			let hash_a = Hash::repeat_byte(1);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(JaegerSpan::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(1).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				view![hash_a]
			).encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer.clone(),
					PeerSet::Collation,
					wire_message.clone(),
				),
			);
		});
	}

	#[test]
	fn relays_collation_protocol_messages() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			network_handle.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer_b.clone(), PeerSet::Collation, ObservedRole::Full).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer_a.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			// peer A gets reported for sending a collation message.

			let collator_protocol_message = protocol_v1::CollatorProtocolMessage::Declare(
				Sr25519Keyring::Alice.public().into()
			);

			let message = protocol_v1::CollationProtocol::CollatorProtocol(
				collator_protocol_message.clone()
			);

			network_handle.peer_message(
				peer_a.clone(),
				PeerSet::Collation,
				WireMessage::ProtocolMessage(message.clone()).encode(),
			).await;

			let actions = network_handle.next_network_actions(1).await;
			assert_network_actions_contains(
				&actions,
				&NetworkAction::ReputationChange(
					peer_a.clone(),
					UNCONNECTED_PEERSET_COST,
				),
			);

			// peer B has the message relayed.

			network_handle.peer_message(
				peer_b.clone(),
				PeerSet::Collation,
				WireMessage::ProtocolMessage(message.clone()).encode(),
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CollatorProtocol(
					CollatorProtocolMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerMessage(p, m)
					)
				) => {
					assert_eq!(p, peer_b);
					assert_eq!(m, collator_protocol_message);
				}
			);
		});
	}

	#[test]
	fn different_views_on_different_peer_sets() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			let view_a = view![Hash::repeat_byte(1)];
			let view_b = view![Hash::repeat_byte(2)];

			network_handle.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view_a.clone()).encode(),
			).await;

			network_handle.peer_message(
				peer.clone(),
				PeerSet::Collation,
				WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(view_b.clone()).encode(),
			).await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), view_a.clone()),
				&mut virtual_overseer,
			).await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), view_b.clone()),
				&mut virtual_overseer,
			).await;
		});
	}

	#[test]
	fn sent_views_include_finalized_number_update() {
		test_harness(|test_harness| async move {
			let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

			let peer_a = PeerId::random();

			network_handle.connect_peer(
				peer_a.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;

			let hash_a = Hash::repeat_byte(1);
			let hash_b = Hash::repeat_byte(2);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::BlockFinalized(hash_a, 1))
			).await;
			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(hash_b, Arc::new(JaegerSpan::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(1).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				View {
					heads: vec![hash_b],
					finalized_number: 1,
				}
			).encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_a.clone(),
					PeerSet::Validation,
					wire_message.clone(),
				),
			);
		});
	}

	#[test]
	fn send_messages_to_peers() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full),
					&mut virtual_overseer,
				).await;

				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			// send a validation protocol message.

			{
				let pov_distribution_message = protocol_v1::PoVDistributionMessage::Awaiting(
					[0; 32].into(),
					vec![[1; 32].into()],
				);

				let message = protocol_v1::ValidationProtocol::PoVDistribution(
					pov_distribution_message.clone(),
				);

				virtual_overseer.send(FromOverseer::Communication {
					msg: NetworkBridgeMessage::SendValidationMessage(
						vec![peer.clone()],
						message.clone(),
					)
				}).await;

				assert_eq!(
					network_handle.next_network_action().await,
					NetworkAction::WriteNotification(
						peer.clone(),
						PeerSet::Validation,
						WireMessage::ProtocolMessage(message).encode(),
					)
				);
			}

			// send a collation protocol message.

			{
				let collator_protocol_message = protocol_v1::CollatorProtocolMessage::Declare(
					Sr25519Keyring::Alice.public().into()
				);

				let message = protocol_v1::CollationProtocol::CollatorProtocol(
					collator_protocol_message.clone()
				);

				virtual_overseer.send(FromOverseer::Communication {
					msg: NetworkBridgeMessage::SendCollationMessage(
						vec![peer.clone()],
						message.clone(),
					)
				}).await;

				assert_eq!(
					network_handle.next_network_action().await,
					NetworkAction::WriteNotification(
						peer.clone(),
						PeerSet::Collation,
						WireMessage::ProtocolMessage(message).encode(),
					)
				);
			}
		});
	}
}
