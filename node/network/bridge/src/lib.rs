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

use parity_scale_codec::{Encode, Decode};
use futures::prelude::*;
use futures::stream::BoxStream;

use sc_network::{
	ObservedRole, ReputationChange, PeerId,
	Event as NetworkEvent,
};
use sp_runtime::ConsensusEngineId;

use messages::{
	NetworkBridgeEvent, NetworkBridgeMessage, FromOverseer, OverseerSignal, AllMessages,
};
use overseer::{Subsystem, SubsystemContext, SpawnedSubsystem};
use node_primitives::{ProtocolId, View};
use polkadot_primitives::{Block, Hash};

use std::collections::hash_map::{HashMap, Entry};
use std::sync::Arc;

const MAX_VIEW_HEADS: usize = 5;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: ConsensusEngineId = *b"dot2";
/// The protocol name.
pub const POLKADOT_PROTOCOL_NAME: &[u8] = b"/polkadot/2";

const MALFORMED_MESSAGE_COST: ReputationChange
	= ReputationChange::new(-500, "Malformed Network-bridge message");
const UNKNOWN_PROTO_COST: ReputationChange
	= ReputationChange::new(-50, "Message sent to unknown protocol");
const MALFORMED_VIEW_COST: ReputationChange
	= ReputationChange::new(-500, "Malformed view");

/// Messages received on the network.
#[derive(Encode, Decode)]
pub enum WireMessage {
	/// A message from a peer on a specific protocol.
	#[codec(index = "1")]
	ProtocolMessage(ProtocolId, Vec<u8>),
	/// A view update from a peer.
	#[codec(index = "2")]
	ViewUpdate(View),
}

/// Information about the notifications protocol. Should be used during network configuration
/// or shortly after startup to register the protocol with the network service.
pub fn notifications_protocol_info() -> (ConsensusEngineId, std::borrow::Cow<'static, [u8]>) {
	(POLKADOT_ENGINE_ID, POLKADOT_PROTOCOL_NAME.into())
}

/// An abstraction over networking for the purposes of this subsystem.
pub trait Network: Clone + Send + Sync + 'static {
	/// Get a stream of all events occurring on the network. This may include events unrelated
	/// to the Polkadot protocol - the user of this function should filter only for events related
	/// to the [`POLKADOT_ENGINE_ID`](POLKADOT_ENGINE_ID).
	fn event_stream(&self) -> BoxStream<NetworkEvent>;

	/// Report a given peer as either beneficial (+) or costly (-) according to the given scalar.
	fn report_peer(&self, who: PeerId, cost_benefit: ReputationChange);

	/// Write a notification to a peer on the [`POLKADOT_ENGINE_ID`](POLKADOT_ENGINE_ID) topic.
	fn write_notification(&self, who: PeerId, message: Vec<u8>);
}

impl Network for Arc<sc_network::NetworkService<Block, Hash>> {
	fn event_stream(&self) -> BoxStream<NetworkEvent> {
		sc_network::NetworkService::event_stream(self, "polkadot-network-bridge").boxed()
	}

	fn report_peer(&self, who: PeerId, cost_benefit: ReputationChange) {
		sc_network::NetworkService::report_peer(self, who, cost_benefit)
	}

	fn write_notification(&self, who: PeerId, message: Vec<u8>) {
		sc_network::NetworkService::write_notification(self, who, POLKADOT_ENGINE_ID, message)
	}
}

/// The network bridge subsystem.
pub struct NetworkBridge<N>(N);

impl<N> NetworkBridge<N> {
	/// Create a new network bridge subsystem with underlying network service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`notifications_protocol_info`](notifications_protocol_info).
	pub fn new(net_service: N) -> Self {
		NetworkBridge(net_service)
	}
}

impl<N: Network> Subsystem<NetworkBridgeMessage> for NetworkBridge<N> {
	fn start(&mut self, ctx: SubsystemContext<NetworkBridgeMessage>) -> SpawnedSubsystem {
		SpawnedSubsystem(run_network(self.0.clone(), ctx).boxed())
	}
}

struct PeerData {
	/// Latest view sent by the peer.
	view: View,
}

enum Action {
	RegisterEventProducer(ProtocolId, fn(NetworkBridgeEvent) -> AllMessages),
	SendMessage(Vec<PeerId>, ProtocolId, Vec<u8>),
	ReportPeer(PeerId, ReputationChange),
	StartWork(Hash),
	StopWork(Hash),

	PeerConnected(PeerId, ObservedRole),
	PeerDisconnected(PeerId),
	PeerMessages(PeerId, Vec<WireMessage>),

	Abort,
}

fn action_from_overseer_message(
	res: overseer::SubsystemResult<FromOverseer<NetworkBridgeMessage>>,
) -> Action {
	match res {
		Ok(FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)))
			=> Action::StartWork(relay_parent),
		Ok(FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)))
			=> Action::StopWork(relay_parent),
		Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => Action::Abort,
		Ok(FromOverseer::Communication { msg }) => match msg {
			NetworkBridgeMessage::RegisterEventProducer(protocol_id, message_producer)
				=>  Action::RegisterEventProducer(protocol_id, message_producer),
			NetworkBridgeMessage::ReportPeer(peer, rep) => Action::ReportPeer(peer, rep),
			NetworkBridgeMessage::SendMessage(peers, protocol, message)
				=> Action::SendMessage(peers, protocol, message),
		},
		Err(e) => {
			log::warn!("Shutting down Network Bridge due to error {:?}", e);
			Action::Abort
		}
	}
}

fn action_from_network_message(event: Option<NetworkEvent>) -> Option<Action> {
	match event {
		None => {
			log::warn!("Shutting down Network Bridge: underlying event stream concluded");
			Some(Action::Abort)
		}
		Some(NetworkEvent::Dht(_)) => None,
		Some(NetworkEvent::NotificationStreamOpened { remote, engine_id, role }) => {
			if engine_id == POLKADOT_ENGINE_ID {
				Some(Action::PeerConnected(remote, role))
			} else {
				None
			}
		}
		Some(NetworkEvent::NotificationStreamClosed { remote, engine_id }) => {
			if engine_id == POLKADOT_ENGINE_ID {
				Some(Action::PeerDisconnected(remote))
			} else {
				None
			}
		}
		Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
			let v: Result<Vec<_>, _> = messages.iter()
				.filter(|(engine_id, _)| engine_id == &POLKADOT_ENGINE_ID)
				.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
				.collect();

			match v {
				Err(_) => Some(Action::ReportPeer(remote, MALFORMED_MESSAGE_COST)),
				Ok(v) => if v.is_empty() {
					None
				} else {
					Some(Action::PeerMessages(remote, v))
				}
			}
		}
	}
}

fn construct_view(live_heads: &[Hash]) -> View {
	View(live_heads.iter().rev().take(MAX_VIEW_HEADS).cloned().collect())
}

async fn dispatch_update_to_all(
	update: NetworkBridgeEvent,
	event_producers: impl IntoIterator<Item=&fn(NetworkBridgeEvent) -> AllMessages>,
	ctx: &mut SubsystemContext<NetworkBridgeMessage>,
) -> overseer::SubsystemResult<()> {
	// collect messages here to avoid the borrow lasting across await boundary.
	let messages: Vec<_> = event_producers.into_iter()
		.map(|producer| producer(update.clone()))
		.collect();

	ctx.send_msgs(messages).await
}

async fn run_network(net: impl Network, mut ctx: SubsystemContext<NetworkBridgeMessage>) {
	let mut event_stream = net.event_stream().fuse();

	// Most recent heads are at the back.
	let mut live_heads = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut local_view = View(Vec::new());

	let mut peers = HashMap::new();
	let mut event_producers = HashMap::new();

	loop {
		let action = {
			let subsystem_next = ctx.recv().fuse();
			let mut net_event_next = event_stream.next().fuse();
			futures::pin_mut!(subsystem_next);

			let action = futures::select! {
				subsystem_msg = subsystem_next => Some(action_from_overseer_message(subsystem_msg)),
				net_event = net_event_next => action_from_network_message(net_event),
			};

			match action {
				Some(a) => a,
				None => continue,
			}
		};

		let update_view = |peers: &HashMap<PeerId, PeerData>, live_heads, local_view: &mut View| {
			let new_view = construct_view(live_heads);
			if *local_view == new_view { return None }
			*local_view = new_view.clone();

			let message = WireMessage::ViewUpdate(new_view).encode();
			for peer in peers.keys().cloned() {
				net.write_notification(peer, message.clone())
			}

			Some(NetworkBridgeEvent::OurViewChange(local_view.clone()))
		};

		match action {
			Action::RegisterEventProducer(protocol_id, event_producer) => {
				// insert only if none present.
				event_producers.entry(protocol_id).or_insert(event_producer);
			}
			Action::SendMessage(peers, protocol, message) => {
				let message = WireMessage::ProtocolMessage(protocol, message).encode();

				for peer in peers.iter().skip(1).cloned() {
					net.write_notification(peer, message.clone());
				}

				if let Some(peer) = peers.first() {
					net.write_notification(peer.clone(), message);
				}
			}
			Action::ReportPeer(peer, rep) => {
				net.report_peer(peer, rep)
			}
			Action::StartWork(relay_parent) => {
				live_heads.push(relay_parent);
				if let Some(view_update) = update_view(&peers, &live_heads, &mut local_view) {
					if let Err(_) = dispatch_update_to_all(
						view_update,
						event_producers.values(),
						&mut ctx,
					).await {
						log::warn!("Aborting - Failure to dispatch messages to overseer");
						return
					}
				}
			}
			Action::StopWork(relay_parent) => {
				live_heads.retain(|h| h != &relay_parent);
				if let Some(view_update) = update_view(&peers, &live_heads, &mut local_view) {
					if let Err(_) = dispatch_update_to_all(
						view_update,
						event_producers.values(),
						&mut ctx,
					).await {
						log::warn!("Aborting - Failure to dispatch messages to overseer");
						return
					}
				}
			}

			Action::PeerConnected(peer, role) => {
				match peers.entry(peer.clone()) {
					Entry::Occupied(_) => continue,
					Entry::Vacant(vacant) => {
						vacant.insert(PeerData {
							view: View(Vec::new()),
						});

						if let Err(_) = dispatch_update_to_all(
							NetworkBridgeEvent::PeerConnected(peer, role),
							event_producers.values(),
							&mut ctx,
						).await {
							log::warn!("Aborting - Failure to dispatch messages to overseer");
							return
						}
					}
				}
			}
			Action::PeerDisconnected(peer) => {
				if peers.remove(&peer).is_some() {
					if let Err(_) = dispatch_update_to_all(
						NetworkBridgeEvent::PeerDisconnected(peer),
						event_producers.values(),
						&mut ctx,
					).await {
						log::warn!("Aborting - Failure to dispatch messages to overseer");
						return
					}
				}
			},
			Action::PeerMessages(peer, messages) => {
				let peer_data = match peers.get_mut(&peer) {
					None => continue,
					Some(d) => d,
				};

				let mut outgoing_messages = Vec::with_capacity(messages.len());
				for message in messages {
					match message {
						WireMessage::ViewUpdate(new_view) => {
							if new_view.0.len() > MAX_VIEW_HEADS {
								net.report_peer(peer.clone(), MALFORMED_VIEW_COST);
								continue
							}

							if new_view == peer_data.view { continue }
							peer_data.view = new_view;

							let update = NetworkBridgeEvent::PeerViewChange(
								peer.clone(),
								peer_data.view.clone(),
							);

							outgoing_messages.extend(
								event_producers.values().map(|producer| producer(update.clone()))
							);
						}
						WireMessage::ProtocolMessage(protocol, message) => {
							let message = match event_producers.get(&protocol) {
								Some(producer) => Some(producer(
									NetworkBridgeEvent::PeerMessage(peer.clone(), message)
								)),
								None => {
									net.report_peer(peer.clone(), UNKNOWN_PROTO_COST);
									None
								}
							};

							if let Some(message) = message {
								outgoing_messages.push(message);
							}
						}
					}
				}

				if let Err(_) = ctx.send_msgs(outgoing_messages).await {
					log::warn!("Aborting - Failure to dispatch messages to overseer");
					return
				}
			},

			Action::Abort => return,
		}
	}
}
