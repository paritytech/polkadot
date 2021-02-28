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

use polkadot_subsystem::{
	ActiveLeavesUpdate, Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemError,
	SubsystemResult, jaeger,
};
use polkadot_subsystem::messages::{
	NetworkBridgeMessage, AllMessages,
	CollatorProtocolMessage, NetworkBridgeEvent,
};
use polkadot_primitives::v1::{Hash, BlockNumber};
use polkadot_node_network_protocol::{
	PeerId, peer_set::PeerSet, View, v1 as protocol_v1, OurView, UnifiedReputationChange as Rep,
};

/// Peer set infos for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::peer_sets_info;

use std::collections::{HashMap, hash_map};
use std::iter::ExactSizeIterator;
use std::sync::Arc;

mod validator_discovery;

/// Internally used `Action` type.
///
/// All requested `NetworkBridgeMessage` user actions  and `NetworkEvent` network messages are
/// translated to `Action` before being processed by `run_network`.
mod action;
use action::{Action, AbortReason};

/// Actual interfacing to the network based on the `Network` trait.
///
/// Defines the `Network` trait with an implementation for an `Arc<NetworkService>`.
mod network;
use network::{Network, send_message};

/// Request multiplexer for combining the multiple request sources into a single `Stream` of `AllMessages`.
mod multiplexer;
pub use multiplexer::RequestMultiplexer;


/// The maximum amount of heads a peer is allowed to have in their view at any time.
///
/// We use the same limit to compute the view sent to peers locally.
const MAX_VIEW_HEADS: usize = 5;


const MALFORMED_MESSAGE_COST: Rep = Rep::CostMajor("Malformed Network-bridge message");
const UNCONNECTED_PEERSET_COST: Rep = Rep::CostMinor("Message sent to un-connected peer-set");
const MALFORMED_VIEW_COST: Rep = Rep::CostMajor("Malformed view");
const EMPTY_VIEW_COST: Rep = Rep::CostMajor("Peer sent us an empty view");

// network bridge log target
const LOG_TARGET: &'static str = "network_bridge";

/// Messages from and to the network.
///
/// As transmitted to and received from subsystems.
#[derive(Debug, Encode, Decode, Clone)]
pub enum WireMessage<M> {
	/// A message from a peer on a specific protocol.
	#[codec(index = 1)]
	ProtocolMessage(M),
	/// A view update from a peer.
	#[codec(index = 2)]
	ViewUpdate(View),
}


/// The network bridge subsystem.
pub struct NetworkBridge<N, AD> {
	/// `Network` trait implementing type.
	network_service: N,
	authority_discovery_service: AD,
	request_multiplexer: RequestMultiplexer,
}

impl<N, AD> NetworkBridge<N, AD> {
	/// Create a new network bridge subsystem with underlying network service and authority discovery service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`peers_sets_info`](peers_sets_info).
	pub fn new(network_service: N, authority_discovery_service: AD, request_multiplexer: RequestMultiplexer) -> Self {
		NetworkBridge {
			network_service,
			authority_discovery_service,
			request_multiplexer,
		}
	}
}

impl<Net, AD, Context> Subsystem<Context> for NetworkBridge<Net, AD>
	where
		Net: Network + validator_discovery::Network + Sync,
		AD: validator_discovery::AuthorityDiscovery,
		Context: SubsystemContext<Message=NetworkBridgeMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		let future = run_network(self, ctx)
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

/// Main driver, processing network events and messages from other subsystems.
#[tracing::instrument(skip(bridge, ctx), fields(subsystem = LOG_TARGET))]
async fn run_network<N, AD>(
	mut bridge: NetworkBridge<N, AD>,
	mut ctx: impl SubsystemContext<Message=NetworkBridgeMessage>,
) -> SubsystemResult<()>
where
	N: Network + validator_discovery::Network,
	AD: validator_discovery::AuthorityDiscovery,
{
	let mut event_stream = bridge.network_service.event_stream().fuse();

	// Most recent heads are at the back.
	let mut live_heads: Vec<(Hash, Arc<jaeger::Span>)> = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut local_view = View::default();
	let mut finalized_number = 0;

	let mut validation_peers: HashMap<PeerId, PeerData> = HashMap::new();
	let mut collation_peers: HashMap<PeerId, PeerData> = HashMap::new();

	let mut validator_discovery = validator_discovery::Service::<N, AD>::new();

	loop {

		let action = {
			let subsystem_next = ctx.recv().fuse();
			let mut net_event_next = event_stream.next().fuse();
			let mut req_res_event_next = bridge.request_multiplexer.next();
			futures::pin_mut!(subsystem_next);

			futures::select! {
				subsystem_msg = subsystem_next => Action::from(subsystem_msg),
				net_event = net_event_next => Action::from(net_event),
				req_res_event = req_res_event_next  => Action::from(req_res_event),
			}
		};

		match action {
			Action::Nop => {}
			Action::Abort(reason) => match reason {
				AbortReason::SubsystemError(err) => {
					tracing::warn!(
						target: LOG_TARGET,
						err = ?err,
						"Shutting down Network Bridge due to error"
					);
					return Err(SubsystemError::Context(format!(
						"Received SubsystemError from overseer: {:?}",
						err
					)));
				}
				AbortReason::EventStreamConcluded => {
					tracing::info!(
						target: LOG_TARGET,
						"Shutting down Network Bridge: underlying request stream concluded"
					);
					return Err(SubsystemError::Context(
						"Incoming network event stream concluded.".to_string(),
					));
				}
				AbortReason::RequestStreamConcluded => {
					tracing::info!(
						target: LOG_TARGET,
						"Shutting down Network Bridge: underlying request stream concluded"
					);
					return Err(SubsystemError::Context(
						"Incoming network request stream concluded".to_string(),
					));
				}
				AbortReason::OverseerConcluded => return Ok(()),
			}

			Action::SendValidationMessages(msgs) => {
				for (peers, msg) in msgs {
					send_message(
							&mut bridge.network_service,
							peers,
							PeerSet::Validation,
							WireMessage::ProtocolMessage(msg),
					).await?
				}
			}

			Action::SendCollationMessages(msgs) => {
				for (peers, msg) in msgs {
					send_message(
							&mut bridge.network_service,
							peers,
							PeerSet::Collation,
							WireMessage::ProtocolMessage(msg),
					).await?
				}
			}

			Action::SendRequests(reqs) => {
				for req in reqs {
					bridge
						.network_service
						.start_request(&mut bridge.authority_discovery_service, req)
						.await;
				}
			},

			Action::ConnectToValidators {
				validator_ids,
				peer_set,
				connected,
			} => {
				tracing::debug!(
					target: LOG_TARGET,
					peer_set = ?peer_set,
					ids = ?validator_ids,
					"Received a validator connection request",
				);
				let (ns, ads) = validator_discovery.on_request(
					validator_ids,
					peer_set,
					connected,
					bridge.network_service,
					bridge.authority_discovery_service,
				).await;
				bridge.network_service = ns;
				bridge.authority_discovery_service = ads;
			},

			Action::ReportPeer(peer, rep) => {
				bridge.network_service.report_peer(peer, rep).await?
			}

			Action::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated }) => {
				live_heads.extend(activated);
				live_heads.retain(|h| !deactivated.contains(&h.0));

				update_our_view(
					&mut bridge.network_service,
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

				validator_discovery.on_peer_connected(
					peer.clone(),
					peer_set,
					&mut bridge.authority_discovery_service,
				).await;

				match peer_map.entry(peer.clone()) {
					hash_map::Entry::Occupied(_) => continue,
					hash_map::Entry::Vacant(vacant) => {
						let _ = vacant.insert(PeerData {
							view: View::default(),
						});

						match peer_set {
							PeerSet::Validation => {
								dispatch_validation_events_to_all(
									vec![
										NetworkBridgeEvent::PeerConnected(peer.clone(), role),
										NetworkBridgeEvent::PeerViewChange(
											peer.clone(),
											View::default(),
										),
									],
									&mut ctx,
								).await;

								send_message(
									&mut bridge.network_service,
									vec![peer],
									PeerSet::Validation,
									WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
										local_view.clone()
									),
								).await?;
							}
							PeerSet::Collation => {
								dispatch_collation_events_to_all(
									vec![
										NetworkBridgeEvent::PeerConnected(peer.clone(), role),
										NetworkBridgeEvent::PeerViewChange(
											peer.clone(),
											View::default(),
										),
									],
									&mut ctx,
								).await;

								send_message(
									&mut bridge.network_service,
									vec![peer],
									PeerSet::Collation,
									WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(
										local_view.clone()
									),
								).await?;
							}
						}
					}
				}
			}
			Action::PeerDisconnected(peer_set, peer) => {
				let peer_map = match peer_set {
					PeerSet::Validation => &mut validation_peers,
					PeerSet::Collation => &mut collation_peers,
				};

				validator_discovery.on_peer_disconnected(&peer, peer_set);

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
						&mut bridge.network_service,
					).await?;

					dispatch_validation_events_to_all(events, &mut ctx).await;
				}

				if !c_messages.is_empty() {
					let events = handle_peer_messages(
						peer.clone(),
						&mut collation_peers,
						c_messages,
						&mut bridge.network_service,
					).await?;

					dispatch_collation_events_to_all(events, &mut ctx).await;
				}
			},
			Action::SendMessage(msg) => ctx.send_message(msg).await,
		}
	}
}

fn construct_view(live_heads: impl DoubleEndedIterator<Item = Hash>, finalized_number: BlockNumber) -> View {
	View::new(
		live_heads.rev().take(MAX_VIEW_HEADS),
		finalized_number
	)
}

#[tracing::instrument(level = "trace", skip(net, ctx, validation_peers, collation_peers), fields(subsystem = LOG_TARGET))]
async fn update_our_view(
	net: &mut impl Network,
	ctx: &mut impl SubsystemContext<Message = NetworkBridgeMessage>,
	live_heads: &[(Hash, Arc<jaeger::Span>)],
	local_view: &mut View,
	finalized_number: BlockNumber,
	validation_peers: &HashMap<PeerId, PeerData>,
	collation_peers: &HashMap<PeerId, PeerData>,
) -> SubsystemResult<()> {
	let new_view = construct_view(live_heads.iter().map(|v| v.0), finalized_number);

	// We only want to send a view update when the heads changed.
	// A change in finalized block number only is _not_ sufficient.
	if local_view.check_heads_eq(&new_view) {
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
				if new_view.len() > MAX_VIEW_HEADS ||
					new_view.finalized_number < peer_data.view.finalized_number
				{
					net.report_peer(
						peer.clone(),
						MALFORMED_VIEW_COST,
					).await?;

					continue
				} else if new_view.is_empty() {
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
	ctx.send_messages(events.into_iter().flat_map(AllMessages::dispatch_iter)).await
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




#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor;
	use futures::stream::BoxStream;
	use std::pin::Pin;
	use std::sync::Arc;

	use std::borrow::Cow;
	use std::collections::HashSet;
	use async_trait::async_trait;
	use parking_lot::Mutex;
	use assert_matches::assert_matches;

	use sc_network::Event as NetworkEvent;

	use polkadot_subsystem::{ActiveLeavesUpdate, FromOverseer, OverseerSignal};
	use polkadot_subsystem::messages::{
		AvailabilityRecoveryMessage,
		ApprovalDistributionMessage,
		BitfieldDistributionMessage,
		PoVDistributionMessage,
		StatementDistributionMessage
	};
	use polkadot_node_subsystem_test_helpers::{
		SingleItemSink, SingleItemStream, TestSubsystemContextHandle,
	};
	use polkadot_node_subsystem_util::metered;
	use polkadot_node_network_protocol::view;
	use sc_network::Multiaddr;
	use sc_network::config::RequestResponseConfig;
	use sp_keyring::Sr25519Keyring;
	use polkadot_primitives::v1::AuthorityDiscoveryId;
	use polkadot_node_network_protocol::{ObservedRole, request_response::request::Requests};

	use crate::network::{Network, NetworkAction};
	use crate::validator_discovery::AuthorityDiscovery;

	// The subsystem's view of the network - only supports a single call to `event_stream`.
	struct TestNetwork {
		net_events: Arc<Mutex<Option<SingleItemStream<NetworkEvent>>>>,
		action_tx: metered::UnboundedMeteredSender<NetworkAction>,
		_req_configs: Vec<RequestResponseConfig>,
	}

	struct TestAuthorityDiscovery;

	// The test's view of the network. This receives updates from the subsystem in the form
	// of `NetworkAction`s.
	struct TestNetworkHandle {
		action_rx: metered::UnboundedMeteredReceiver<NetworkAction>,
		net_tx: SingleItemSink<NetworkEvent>,
	}

	fn new_test_network(req_configs: Vec<RequestResponseConfig>) -> (
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
				_req_configs: req_configs,
			},
			TestNetworkHandle {
				action_rx,
				net_tx,
			},
			TestAuthorityDiscovery,
		)
	}

	#[async_trait]
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

		async fn start_request<AD: AuthorityDiscovery>(&self, _: &mut AD, _: Requests) {
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
				protocol: peer_set.into_protocol_name(),
				role: role.into(),
			}).await;
		}

		async fn disconnect_peer(&mut self, peer: PeerId, peer_set: PeerSet) {
			self.send_network_event(NetworkEvent::NotificationStreamClosed {
				remote: peer,
				protocol: peer_set.into_protocol_name(),
			}).await;
		}

		async fn peer_message(&mut self, peer: PeerId, peer_set: PeerSet, message: Vec<u8>) {
			self.send_network_event(NetworkEvent::NotificationsReceived {
				remote: peer,
				messages: vec![(peer_set.into_protocol_name(), message.into())],
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
		let (request_multiplexer, req_configs) = RequestMultiplexer::new();
		let (network, network_handle, discovery) = new_test_network(req_configs);
		let (context, virtual_overseer) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bridge = NetworkBridge {
			network_service: network,
			authority_discovery_service: discovery,
			request_multiplexer,
		};

		let network_bridge = run_network(
			bridge,
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
		// Ordering must match the enum variant order
		// in `AllMessages`.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::NetworkBridgeUpdateV1(e)
			) if e == event.focus().expect("could not focus message")
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityRecovery(
				AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(e)
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
			AllMessages::ApprovalDistribution(
				ApprovalDistributionMessage::NetworkBridgeUpdateV1(e)
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
	fn send_our_view_upon_connection() {
		test_harness(|test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			let head = Hash::repeat_byte(1);
			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(head, Arc::new(jaeger::Span::Disabled)),
				))
			).await;

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			let view = view![head];
			let actions = network_handle.next_network_actions(2).await;
			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer.clone(),
					PeerSet::Validation,
					WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
						view.clone(),
					).encode(),
				),
			);
			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer.clone(),
					PeerSet::Collation,
					WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(
						view.clone(),
					).encode(),
				),
			);
		});
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
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(jaeger::Span::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(4).await;
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
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(jaeger::Span::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(4).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				View::new(vec![hash_a], 5)
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
					ActiveLeavesUpdate::start_work(hash_a, Arc::new(jaeger::Span::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(3).await;
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

			let actions = network_handle.next_network_actions(3).await;
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
					ActiveLeavesUpdate::start_work(hash_b, Arc::new(jaeger::Span::Disabled)),
				))
			).await;

			let actions = network_handle.next_network_actions(2).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				View::new(vec![hash_b], 1)
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
	fn view_finalized_number_can_not_go_down() {
		test_harness(|test_harness| async move {
			let TestHarness { mut network_handle, .. } = test_harness;

			let peer_a = PeerId::random();

			network_handle.connect_peer(
				peer_a.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;

			network_handle.peer_message(
				peer_a.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
					View::new(vec![Hash::repeat_byte(0x01)], 1),
				).encode(),
			).await;

			network_handle.peer_message(
				peer_a.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
					View::new(vec![], 0),
				).encode(),
			).await;

			let actions = network_handle.next_network_actions(2).await;
			assert_network_actions_contains(
				&actions,
				&NetworkAction::ReputationChange(
					peer_a.clone(),
					MALFORMED_VIEW_COST,
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

			// consume peer view changes
			{
				let _peer_view_changes = network_handle.next_network_actions(2).await;
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

	#[test]
	fn spread_event_to_subsystems_is_up_to_date() {
		// Number of subsystems expected to be interested in a network event,
		// and hence the network event broadcasted to.
		const EXPECTED_COUNT: usize = 5;

		let mut cnt = 0_usize;
		for msg in AllMessages::dispatch_iter(NetworkBridgeEvent::PeerDisconnected(PeerId::random())) {
			match msg {
				AllMessages::CandidateValidation(_) => unreachable!("Not interested in network events"),
				AllMessages::CandidateBacking(_) => unreachable!("Not interested in network events"),
				AllMessages::CandidateSelection(_) => unreachable!("Not interested in network events"),
				AllMessages::ChainApi(_) => unreachable!("Not interested in network events"),
				AllMessages::CollatorProtocol(_) => unreachable!("Not interested in network events"),
				AllMessages::StatementDistribution(_) => { cnt += 1; }
				AllMessages::AvailabilityDistribution(_) => unreachable!("Not interested in network events"),
				AllMessages::AvailabilityRecovery(_) => { cnt += 1; }
				AllMessages::BitfieldDistribution(_) => { cnt += 1; }
				AllMessages::BitfieldSigning(_) => unreachable!("Not interested in network events"),
				AllMessages::Provisioner(_) => unreachable!("Not interested in network events"),
				AllMessages::PoVDistribution(_) => { cnt += 1; }
				AllMessages::RuntimeApi(_) => unreachable!("Not interested in network events"),
				AllMessages::AvailabilityStore(_) => unreachable!("Not interested in network events"),
				AllMessages::NetworkBridge(_) => unreachable!("Not interested in network events"),
				AllMessages::CollationGeneration(_) => unreachable!("Not interested in network events"),
				AllMessages::ApprovalVoting(_) => unreachable!("Not interested in network events"),
				AllMessages::ApprovalDistribution(_) => { cnt += 1; }
				AllMessages::GossipSupport(_) => unreachable!("Not interested in network events"),
				// Add variants here as needed, `{ cnt += 1; }` for those that need to be
				// notified, `unreachable!()` for those that should not.
			}
		}
		assert_eq!(cnt, EXPECTED_COUNT);
	}
}
