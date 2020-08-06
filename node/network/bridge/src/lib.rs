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
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::channel::oneshot;

use sc_network::Event as NetworkEvent;
use sp_runtime::ConsensusEngineId;

use polkadot_subsystem::{
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemError,
	SubsystemResult,
};
use polkadot_subsystem::messages::{
	NetworkBridgeMessage, AllMessages, AvailabilityDistributionMessage,
	BitfieldDistributionMessage, PoVDistributionMessage, StatementDistributionMessage,
	CollatorProtocolMessage,
};
use polkadot_primitives::v1::{Block, Hash, ValidatorId};
use polkadot_node_network_protocol::{
	ObservedRole, ReputationChange, PeerId, PeerSet, View, NetworkBridgeEvent, v1 as protocol_v1
};

use std::collections::hash_map::{HashMap, Entry as HEntry};
use std::iter::ExactSizeIterator;
use std::pin::Pin;
use std::sync::Arc;

/// The maximum amount of heads a peer is allowed to have in their view at any time.
///
/// We use the same limit to compute the view sent to peers locally.
const MAX_VIEW_HEADS: usize = 5;

/// The engine ID of the validation protocol.
pub const VALIDATION_PROTOCOL_ID: ConsensusEngineId = *b"pvn1";
/// The protocol name for the validation peer-set.
pub const VALIDATION_PROTOCOL_NAME: &[u8] = b"/polkadot/validation/1";
/// The engine ID of the collation protocol.
pub const COLLATION_PROTOCOL_ID: ConsensusEngineId = *b"pcn1";
/// The protocol name for the collation peer-set.
pub const COLLATION_PROTOCOL_NAME: &[u8] = b"/polkadot/collation/1";

const MALFORMED_MESSAGE_COST: ReputationChange
	= ReputationChange::new(-500, "Malformed Network-bridge message");
const UNKNOWN_PROTO_COST: ReputationChange
	= ReputationChange::new(-50, "Message sent to unknown protocol");
const MALFORMED_VIEW_COST: ReputationChange
	= ReputationChange::new(-500, "Malformed view");

// network bridge log target
const TARGET: &'static str = "network_bridge";

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

/// Information about the notifications protocol. Should be used during network configuration
/// or shortly after startup to register the protocol with the network service.
pub fn notifications_protocol_info() -> Vec<(ConsensusEngineId, std::borrow::Cow<'static, [u8]>)> {
	vec![
		(VALIDATION_PROTOCOL_ID, VALIDATION_PROTOCOL_NAME.into()),
		(COLLATION_PROTOCOL_ID, COLLATION_PROTOCOL_NAME.into()),
	]
}

/// An action to be carried out by the network.
#[derive(PartialEq)]
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
	/// to the [`VALIDATION_PROTOCOL_ID`](VALIDATION_PROTOCOL_ID)
	/// or [`COLLATION_PROTOCOL_ID`](COLLATION_PROTOCOL_ID)
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
					NetworkAction::ReputationChange(peer, cost_benefit) => self.0.report_peer(
						peer,
						cost_benefit,
					),
					NetworkAction::WriteNotification(peer, peer_set, message) => {
						match peer_set {
							PeerSet::Validation => self.0.write_notification(
								peer,
								VALIDATION_PROTOCOL_ID,
								message,
							),
							PeerSet::Collation => self.0.write_notification(
								peer,
								COLLATION_PROTOCOL_ID,
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

impl<Net, Context> Subsystem<Context> for NetworkBridge<Net>
	where
		Net: Network,
		Context: SubsystemContext<Message=NetworkBridgeMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		SpawnedSubsystem {
			name: "network-bridge-subsystem",
			future: run_network(self.0, ctx).map(|_| ()).boxed(),
		}
	}
}

struct PeerData {
	/// Latest view sent by the peer.
	view: View,
	/// The role of the peer.
	role: ObservedRole,
}

#[derive(Debug)]
enum Action {
	SendValidationMessage(Vec<PeerId>, protocol_v1::ValidationProtocol),
	SendCollationMessage(Vec<PeerId>, protocol_v1::CollationProtocol),
	ConnectToValidators(PeerSet, Vec<ValidatorId>, oneshot::Sender<Vec<(ValidatorId, PeerId)>>),
	ReportPeer(PeerId, ReputationChange),

	ActiveLeaves(ActiveLeavesUpdate),

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

fn action_from_overseer_message(
	res: polkadot_subsystem::SubsystemResult<FromOverseer<NetworkBridgeMessage>>,
) -> Action {
	match res {
		Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(active_leaves)))
			=> Action::ActiveLeaves(active_leaves),
		Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => Action::Abort,
		Ok(FromOverseer::Communication { msg }) => match msg {
			NetworkBridgeMessage::ReportPeer(peer, rep) => Action::ReportPeer(peer, rep),
			NetworkBridgeMessage::SendValidationMessage(peers, msg)
				=> Action::SendValidationMessage(peers, msg),
			NetworkBridgeMessage::SendCollationMessage(peers, msg)
				=> Action::SendCollationMessage(peers, msg),
			NetworkBridgeMessage::ConnectToValidators(peer_set, validators, res)
				=> Action::ConnectToValidators(peer_set, validators, res),
		},
		Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(_)))
			=> Action::Nop,
		Err(e) => {
			log::warn!(target: TARGET, "Shutting down Network Bridge due to error {:?}", e);
			Action::Abort
		}
	}
}

fn action_from_network_message(event: Option<NetworkEvent>) -> Action {
	match event {
		None => {
			log::info!(target: TARGET, "Shutting down Network Bridge: underlying event stream concluded");
			Action::Abort
		}
		Some(NetworkEvent::Dht(_)) => Action::Nop,
		Some(NetworkEvent::NotificationStreamOpened { remote, engine_id, role }) => {
			match engine_id {
				x if x == VALIDATION_PROTOCOL_ID
					=> Action::PeerConnected(PeerSet::Validation, remote, role),
				x if x == COLLATION_PROTOCOL_ID
					=> Action::PeerConnected(PeerSet::Collation, remote, role),
				_ => Action::Nop,
			}
		}
		Some(NetworkEvent::NotificationStreamClosed { remote, engine_id }) => {
			match engine_id {
				x if x == VALIDATION_PROTOCOL_ID
					=> Action::PeerDisconnected(PeerSet::Validation, remote),
				x if x == COLLATION_PROTOCOL_ID
					=> Action::PeerDisconnected(PeerSet::Collation, remote),
				_ => Action::Nop,
			}
		}
		Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
			let v_messages: Result<Vec<_>, _> = messages.iter()
				.filter(|(engine_id, _)| engine_id == &VALIDATION_PROTOCOL_ID)
				.map(|(_, msg_bytes)| WireMessage::decode(&mut msg_bytes.as_ref()))
				.collect();

			let v_messages = match v_messages {
				Err(_) => return Action::ReportPeer(remote, MALFORMED_MESSAGE_COST),
				Ok(v) => v,
			};

			let c_messages: Result<Vec<_>, _> = messages.iter()
				.filter(|(engine_id, _)| engine_id == &COLLATION_PROTOCOL_ID)
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

fn construct_view(live_heads: &[Hash]) -> View {
	View(live_heads.iter().rev().take(MAX_VIEW_HEADS).cloned().collect())
}

async fn update_view(
	net: &mut impl Network,
	ctx: &mut impl SubsystemContext<Message = NetworkBridgeMessage>,
	live_heads: &[Hash],
	local_view: &mut View,
	validation_peers: &HashMap<PeerId, PeerData>,
	collation_peers: &HashMap<PeerId, PeerData>,
) -> SubsystemResult<()> {
	let new_view = construct_view(live_heads);
	if *local_view == new_view { return Ok(())  }

	*local_view = new_view.clone();

	send_validation_message(
		net,
		validation_peers.keys().cloned(),
		WireMessage::ViewUpdate(new_view.clone()),
	).await?;

	send_collation_message(
		net,
		collation_peers.keys().cloned(),
		WireMessage::ViewUpdate(new_view.clone()),
	).await?;

	if let Err(e) = dispatch_validation_event_to_all(
		NetworkBridgeEvent::OurViewChange(new_view.clone()),
		ctx,
	).await {
		log::warn!(target: TARGET, "Aborting - Failure to dispatch messages to overseer");
		return Err(e)
	}

	if let Err(e) = dispatch_collation_event_to_all(
		NetworkBridgeEvent::OurViewChange(new_view.clone()),
		ctx,
	).await {
		log::warn!(target: TARGET, "Aborting - Failure to dispatch messages to overseer");
		return Err(e)
	}

	Ok(())
}

async fn send_validation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::ValidationProtocol>,
) -> SubsystemResult<()>
	where I: IntoIterator<Item=PeerId>, I::IntoIter: ExactSizeIterator
{
	send_message(net, peers, PeerSet::Validation, message).await
}

async fn send_collation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::CollationProtocol>,
) -> SubsystemResult<()>
	where I: IntoIterator<Item=PeerId>, I::IntoIter: ExactSizeIterator
{
	send_message(net, peers, PeerSet::Collation, message).await
}

async fn send_message<M: Encode + Clone, I>(
	net: &mut impl Network,
	peers: I,
	peer_set: PeerSet,
	message: WireMessage<M>,
) -> SubsystemResult<()>
	where I: IntoIterator<Item=PeerId>, I::IntoIter: ExactSizeIterator,
{
	let mut message_producer = stream::iter({
		let mut peers = peers.into_iter();
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
) -> SubsystemResult<()> {
	let messages = vec![
		event.focus().ok().map(|m| AllMessages::AvailabilityDistribution(
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(m)
		)),
		event.focus().ok().map(|m| AllMessages::BitfieldDistribution(
			BitfieldDistributionMessage::NetworkBridgeUpdateV1(m)
		)),
		event.focus().ok().map(|m| AllMessages::PoVDistribution(
			PoVDistributionMessage::NetworkBridgeUpdateV1(m)
		)),
		event.focus().ok().map(|m| AllMessages::StatementDistribution(
			StatementDistributionMessage::NetworkBridgeUpdateV1(m)
		)),
	];

	ctx.send_messages(messages.into_iter().filter_map(|x| x)).await
}

async fn dispatch_collation_event_to_all(
	event: NetworkBridgeEvent<protocol_v1::CollationProtocol>,
	ctx: &mut impl SubsystemContext<Message=NetworkBridgeMessage>,
) -> SubsystemResult<()> {
	let message = event.focus().ok().map(|m| AllMessages::CollatorProtocol(
		CollatorProtocolMessage::NetworkBridgeUpdateV1(m)
	));

	match message {
		Some(m) => ctx.send_message(m).await,
		None => Ok(()), // technically unreachable due to single variant.
	}
}

async fn run_network<N: Network>(
	mut net: N,
	mut ctx: impl SubsystemContext<Message=NetworkBridgeMessage>,
) -> SubsystemResult<()> {
	let mut event_stream = net.event_stream().fuse();

	// Most recent heads are at the back.
	let mut live_heads: Vec<Hash> = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut local_view = View(Vec::new());

	let mut validation_peers: HashMap<PeerId, PeerData> = HashMap::new();
	let mut collation_peers: HashMap<PeerId, PeerData> = HashMap::new();

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

			Action::SendValidationMessage(peers, msg) => send_message(
					&mut net,
					peers,
					PeerSet::Validation,
					WireMessage::ProtocolMessage(msg),
			).await?,

			Action::SendCollationMessage(peers, msg) => send_message(
					&mut net,
					peers,
					PeerSet::Collation,
					WireMessage::ProtocolMessage(msg),
			).await?,

			Action::ConnectToValidators(_peer_set, _validators, _res) => {
				// TODO: https://github.com/paritytech/polkadot/issues/1461
			}

			Action::ReportPeer(peer, rep) => net.report_peer(peer, rep).await?,

			Action::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated }) => {
				live_heads.extend(activated);
				live_heads.retain(|h| !deactivated.contains(h));

				update_view(
					&mut net,
					&mut ctx,
					&live_heads,
					&mut local_view,
					&validation_peers,
					&collation_peers,
				).await?;
			}

			_ => {} // TODO [now]: exhaustive match
		}

		// match action {
		// 	Action::RegisterEventProducer(protocol_id, event_producer) => {
		// 		// insert only if none present.
		// 		if let BEntry::Vacant(entry) = event_producers.entry(protocol_id) {
		// 			let event_producer = entry.insert(event_producer);

		// 			// send the event producer information on all connected peers.
		// 			let mut messages = Vec::with_capacity(peers.len() * 2);
		// 			for (peer, data) in &peers {
		// 				messages.push(event_producer(
		// 					NetworkBridgeEvent::PeerConnected(peer.clone(), data.role.clone())
		// 				));

		// 				messages.push(event_producer(
		// 					NetworkBridgeEvent::PeerViewChange(peer.clone(), data.view.clone())
		// 				));
		// 			}

		// 			ctx.send_messages(messages).await?;
		// 		}
		// 	}
		// 	Action::ReportPeer(peer, rep) => {
		// 		net.report_peer(peer, rep).await?;
		// 	}
		// 	Action::PeerConnected(peer, role) => {
		// 		match peers.entry(peer.clone()) {
		// 			HEntry::Occupied(_) => continue,
		// 			HEntry::Vacant(vacant) => {
		// 				vacant.insert(PeerData {
		// 					view: View(Vec::new()),
		// 					role: role.clone(),
		// 				});

		// 				if let Err(e) = dispatch_update_to_all(
		// 					NetworkBridgeEvent::PeerConnected(peer, role),
		// 					event_producers.values(),
		// 					&mut ctx,
		// 				).await {
		// 					log::warn!("Aborting - Failure to dispatch messages to overseer");
		// 					return Err(e)
		// 				}
		// 			}
		// 		}
		// 	}
		// 	Action::PeerDisconnected(peer) => {
		// 		if peers.remove(&peer).is_some() {
		// 			if let Err(e) = dispatch_update_to_all(
		// 				NetworkBridgeEvent::PeerDisconnected(peer),
		// 				event_producers.values(),
		// 				&mut ctx,
		// 			).await {
		// 				log::warn!(target: TARGET, "Aborting - Failure to dispatch messages to overseer");
		// 				return Err(e)
		// 			}
		// 		}
		// 	},
		// 	Action::PeerMessages(peer, messages) => {
		// 		let peer_data = match peers.get_mut(&peer) {
		// 			None => continue,
		// 			Some(d) => d,
		// 		};

		// 		let mut outgoing_messages = Vec::with_capacity(messages.len());
		// 		for message in messages {
		// 			match message {
		// 				WireMessage::ViewUpdate(new_view) => {
		// 					if new_view.0.len() > MAX_VIEW_HEADS {
		// 						net.report_peer(
		// 							peer.clone(),
		// 							MALFORMED_VIEW_COST,
		// 						).await?;

		// 						continue
		// 					}

		// 					if new_view == peer_data.view { continue }
		// 					peer_data.view = new_view;

		// 					let update = NetworkBridgeEvent::PeerViewChange(
		// 						peer.clone(),
		// 						peer_data.view.clone(),
		// 					);

		// 					outgoing_messages.extend(
		// 						event_producers.values().map(|producer| producer(update.clone()))
		// 					);
		// 				}
		// 				WireMessage::ProtocolMessage(protocol, message) => {
		// 					let message = match event_producers.get(&protocol) {
		// 						Some(producer) => Some(producer(
		// 							NetworkBridgeEvent::PeerMessage(peer.clone(), message)
		// 						)),
		// 						None => {
		// 							net.report_peer(
		// 								peer.clone(),
		// 								UNKNOWN_PROTO_COST,
		// 							).await?;

		// 							None
		// 						}
		// 					};

		// 					if let Some(message) = message {
		// 						outgoing_messages.push(message);
		// 					}
		// 				}
		// 			}
		// 		}

		// 		let send_messages = ctx.send_messages(outgoing_messages);
		// 		if let Err(e) = send_messages.await {
		// 			log::warn!(target: TARGET, "Aborting - Failure to dispatch messages to overseer");
		// 			return Err(e)
		// 		}
		// 	},

		// 	Action::Abort => return Ok(()),
		// 	Action::Nop => (),
		// }
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::channel::mpsc;
	use futures::executor;

	use std::sync::Arc;
	use parking_lot::Mutex;
	use assert_matches::assert_matches;

	use polkadot_subsystem::messages::{StatementDistributionMessage, BitfieldDistributionMessage};
	use polkadot_subsystem::test_helpers::{SingleItemSink, SingleItemStream};

	// The subsystem's view of the network - only supports a single call to `event_stream`.
	struct TestNetwork {
		net_events: Arc<Mutex<Option<SingleItemStream<NetworkEvent>>>>,
		action_tx: mpsc::UnboundedSender<NetworkAction>,
	}

	// The test's view of the network. This receives updates from the subsystem in the form
	// of `NetworkAction`s.
	struct TestNetworkHandle {
		action_rx: mpsc::UnboundedReceiver<NetworkAction>,
		net_tx: SingleItemSink<NetworkEvent>,
	}

	fn new_test_network() -> (
		TestNetwork,
		TestNetworkHandle,
	) {
		let (net_tx, net_rx) = polkadot_subsystem::test_helpers::single_item_sink();
		let (action_tx, action_rx) = mpsc::unbounded();

		(
			TestNetwork {
				net_events: Arc::new(Mutex::new(Some(net_rx))),
				action_tx,
			},
			TestNetworkHandle {
				action_rx,
				net_tx,
			},
		)
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

		async fn connect_peer(&mut self, peer: PeerId, role: ObservedRole) {
			self.send_network_event(NetworkEvent::NotificationStreamOpened {
				remote: peer,
				engine_id: VALIDATION_PROTOCOL_ID,
				role,
			}).await;
		}

		async fn disconnect_peer(&mut self, peer: PeerId) {
			self.send_network_event(NetworkEvent::NotificationStreamClosed {
				remote: peer,
				engine_id: VALIDATION_PROTOCOL_ID,
			}).await;
		}

		async fn peer_message(&mut self, peer: PeerId, message: Vec<u8>) {
			self.send_network_event(NetworkEvent::NotificationsReceived {
				remote: peer,
				messages: vec![(VALIDATION_PROTOCOL_ID, message.into())],
			}).await;
		}

		async fn send_network_event(&mut self, event: NetworkEvent) {
			self.net_tx.send(event).await.expect("subsystem concluded early");
		}
	}

	// network actions are sensitive to ordering of `PeerId`s within a `HashMap`, so
	// we need to use this to prevent fragile reliance on peer ordering.
	fn network_actions_contains(actions: &[NetworkAction], action: &NetworkAction) -> bool {
		actions.iter().find(|&x| x == action).is_some()
	}

	struct TestHarness {
		network_handle: TestNetworkHandle,
		virtual_overseer: polkadot_subsystem::test_helpers::TestSubsystemContextHandle<NetworkBridgeMessage>,
	}

	fn test_harness<T: Future<Output=()>>(test: impl FnOnce(TestHarness) -> T) {
		let pool = sp_core::testing::TaskExecutor::new();
		let (network, network_handle) = new_test_network();
		let (context, virtual_overseer) = polkadot_subsystem::test_helpers::make_subsystem_context(pool);

		let network_bridge = run_network(
			network,
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

		executor::block_on(future::select(test_fut, network_bridge));
	}

	#[test]
	fn sends_view_updates_to_peers() {
		test_harness(|test_harness| async move {
			let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			network_handle.connect_peer(peer_a.clone(), ObservedRole::Full).await;
			network_handle.connect_peer(peer_b.clone(), ObservedRole::Full).await;

			let hash_a = Hash::from([1; 32]);

			virtual_overseer.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(hash_a)))).await;

			let actions = network_handle.next_network_actions(2).await;
			let wire_message = WireMessage::ViewUpdate(View(vec![hash_a])).encode();
			assert!(network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(peer_a, wire_message.clone()),
			));

			assert!(network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(peer_b, wire_message.clone()),
			));
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

			let proto_statement = *b"abcd";
			let proto_bitfield = *b"wxyz";

			network_handle.connect_peer(peer.clone(), ObservedRole::Full).await;

			virtual_overseer.send(FromOverseer::Communication {
				msg: NetworkBridgeMessage::RegisterEventProducer(
					proto_statement,
					|event| AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(event)
					)
				),
			}).await;

			virtual_overseer.send(FromOverseer::Communication {
				msg: NetworkBridgeMessage::RegisterEventProducer(
					proto_bitfield,
					|event| AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(event)
					)
				),
			}).await;

			let view = View(vec![Hash::from([1u8; 32])]);

			// bridge will inform about all previously-connected peers.
			{
				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerConnected(p, ObservedRole::Full)
						)
					) if p == peer
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerViewChange(p, v)
						)
					) if p == peer && v == View(Default::default())
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerConnected(p, ObservedRole::Full)
						)
					) if p == peer
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerViewChange(p, v)
						)
					) if p == peer && v == View(Default::default())
				);
			}

			network_handle.peer_message(
				peer.clone(),
				WireMessage::ViewUpdate(view.clone()).encode(),
			).await;

			// statement distribution message comes first because handlers are ordered by
			// protocol ID.

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerViewChange(p, v)
					)
				) => {
					assert_eq!(p, peer);
					assert_eq!(v, view);
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::BitfieldDistribution(
					BitfieldDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerViewChange(p, v)
					)
				) => {
					assert_eq!(p, peer);
					assert_eq!(v, view);
				}
			);
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

			let proto_statement = *b"abcd";
			let proto_bitfield = *b"wxyz";

			network_handle.connect_peer(peer.clone(), ObservedRole::Full).await;

			virtual_overseer.send(FromOverseer::Communication {
				msg: NetworkBridgeMessage::RegisterEventProducer(
					proto_statement,
					|event| AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(event)
					)
				),
			}).await;

			virtual_overseer.send(FromOverseer::Communication {
				msg: NetworkBridgeMessage::RegisterEventProducer(
					proto_bitfield,
					|event| AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(event)
					)
				),
			}).await;

			// bridge will inform about all previously-connected peers.
			{
				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerConnected(p, ObservedRole::Full)
						)
					) if p == peer
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::StatementDistribution(
						StatementDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerViewChange(p, v)
						)
					) if p == peer && v == View(Default::default())
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerConnected(p, ObservedRole::Full)
						)
					) if p == peer
				);

				assert_matches!(
					virtual_overseer.recv().await,
					AllMessages::BitfieldDistribution(
						BitfieldDistributionMessage::NetworkBridgeUpdate(
							NetworkBridgeEvent::PeerViewChange(p, v)
						)
					) if p == peer && v == View(Default::default())
				);
			}

			let payload = vec![1, 2, 3];

			network_handle.peer_message(
				peer.clone(),
				WireMessage::ProtocolMessage(proto_statement, payload.clone()).encode(),
			).await;

			network_handle.disconnect_peer(peer.clone()).await;

			// statement distribution message comes first because handlers are ordered by
			// protocol ID, and then a disconnection event comes - indicating that the message
			// was only sent to the correct protocol.

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerMessage(p, m)
					)
				) => {
					assert_eq!(p, peer);
					assert_eq!(m, payload);
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerDisconnected(p)
					)
				) => {
					assert_eq!(p, peer);
				}
			);
		});
	}
}
