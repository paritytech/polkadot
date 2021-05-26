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
use parking_lot::Mutex;
use futures::prelude::*;
use futures::stream::BoxStream;
use sc_network::Event as NetworkEvent;
use sp_consensus::SyncOracle;

use polkadot_subsystem::{
	ActivatedLeaf, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError, SubsystemResult, SubsystemSender,
	messages::StatementDistributionMessage
};
use polkadot_subsystem::messages::{
	NetworkBridgeMessage, AllMessages,
	CollatorProtocolMessage, NetworkBridgeEvent,
};
use polkadot_primitives::v1::{Hash, BlockNumber};
use polkadot_node_network_protocol::{
	PeerId, peer_set::PeerSet, View, v1 as protocol_v1, OurView, UnifiedReputationChange as Rep,
	ObservedRole,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};

/// Peer set infos for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::{peer_sets_info, IsAuthority};

use std::collections::{HashMap, hash_map};
use std::iter::ExactSizeIterator;
use std::sync::Arc;

mod validator_discovery;

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
const LOG_TARGET: &'static str = "parachain::network-bridge";

/// Metrics for the network bridge.
#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_peer_connected(&self, peer_set: PeerSet) {
		self.0.as_ref().map(|metrics| metrics
			.connected_events
			.with_label_values(&[peer_set.get_protocol_name_static()])
			.inc()
		);
	}

	fn on_peer_disconnected(&self, peer_set: PeerSet) {
		self.0.as_ref().map(|metrics| metrics
			.disconnected_events
			.with_label_values(&[peer_set.get_protocol_name_static()])
			.inc()
		);
	}

	fn note_peer_count(&self, peer_set: PeerSet, count: usize) {
		self.0.as_ref().map(|metrics| metrics
			.peer_count
			.with_label_values(&[peer_set.get_protocol_name_static()])
			.set(count as u64)
		);
	}

	fn on_notification_received(&self, peer_set: PeerSet, size: usize) {
		if let Some(metrics) = self.0.as_ref() {
			metrics.notifications_received
				.with_label_values(&[peer_set.get_protocol_name_static()])
				.inc();

			metrics.bytes_received
				.with_label_values(&[peer_set.get_protocol_name_static()])
				.inc_by(size as u64);
		}
	}

	fn on_notification_sent(&self, peer_set: PeerSet, size: usize, to_peers: usize) {
		if let Some(metrics) = self.0.as_ref() {
			metrics.notifications_sent
				.with_label_values(&[peer_set.get_protocol_name_static()])
				.inc_by(to_peers as u64);

			metrics.bytes_sent
				.with_label_values(&[peer_set.get_protocol_name_static()])
				.inc_by((size * to_peers) as u64);
		}
	}

	fn note_desired_peer_count(&self, peer_set: PeerSet, size: usize) {
		self.0.as_ref().map(|metrics| metrics
			.desired_peer_count
			.with_label_values(&[peer_set.get_protocol_name_static()])
			.set(size as u64)
		);
	}
}

#[derive(Clone)]
struct MetricsInner {
	peer_count: prometheus::GaugeVec<prometheus::U64>,
	connected_events: prometheus::CounterVec<prometheus::U64>,
	disconnected_events: prometheus::CounterVec<prometheus::U64>,
	desired_peer_count: prometheus::GaugeVec<prometheus::U64>,

	notifications_received: prometheus::CounterVec<prometheus::U64>,
	notifications_sent: prometheus::CounterVec<prometheus::U64>,

	bytes_received: prometheus::CounterVec<prometheus::U64>,
	bytes_sent: prometheus::CounterVec<prometheus::U64>,
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry)
		-> std::result::Result<Self, prometheus::PrometheusError>
	{
		let metrics = MetricsInner {
			peer_count: prometheus::register(
				prometheus::GaugeVec::new(
					prometheus::Opts::new(
						"parachain_peer_count",
						"The number of peers on a parachain-related peer-set",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			connected_events: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_peer_connect_events_total",
						"The number of peer connect events on a parachain notifications protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			disconnected_events: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_peer_disconnect_events_total",
						"The number of peer disconnect events on a parachain notifications protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			desired_peer_count: prometheus::register(
				prometheus::GaugeVec::new(
					prometheus::Opts::new(
						"parachain_desired_peer_count",
						"The number of peers that the local node is expected to connect to on a parachain-related peer-set",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			notifications_received: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_notifications_received_total",
						"The number of notifications received on a parachain protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			notifications_sent: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_notifications_sent_total",
						"The number of notifications sent on a parachain protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			bytes_received: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_notification_bytes_received_total",
						"The number of bytes received on a parachain notification protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
			bytes_sent: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_notification_bytes_sent_total",
						"The number of bytes sent on a parachain notification protocol",
					),
					&["protocol"]
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}

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
	sync_oracle: Box<dyn SyncOracle + Send>,
	metrics: Metrics,
}

impl<N, AD> NetworkBridge<N, AD> {
	/// Create a new network bridge subsystem with underlying network service and authority discovery service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`peers_sets_info`](peers_sets_info).
	pub fn new(
		network_service: N,
		authority_discovery_service: AD,
		request_multiplexer: RequestMultiplexer,
		sync_oracle: Box<dyn SyncOracle + Send>,
		metrics: Metrics,
	) -> Self {
		NetworkBridge {
			network_service,
			authority_discovery_service,
			request_multiplexer,
			sync_oracle,
			metrics,
		}
	}
}

impl<Net, AD, Context> Subsystem<Context> for NetworkBridge<Net, AD>
	where
		Net: Network + Sync,
		AD: validator_discovery::AuthorityDiscovery,
		Context: SubsystemContext<Message=NetworkBridgeMessage>,
{
	fn start(mut self, ctx: Context) -> SpawnedSubsystem {
		// The stream of networking events has to be created at initialization, otherwise the
		// networking might open connections before the stream of events has been grabbed.
		let network_stream = self.network_service.event_stream();

		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		let future = run_network(self, ctx, network_stream)
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
	/// The Latest view sent by the peer.
	view: View,
}

#[derive(Debug)]
enum UnexpectedAbort {
	/// Received error from overseer:
	SubsystemError(polkadot_subsystem::SubsystemError),
	/// The stream of incoming events concluded.
	EventStreamConcluded,
	/// The stream of incoming requests concluded.
	RequestStreamConcluded,
}

impl From<SubsystemError> for UnexpectedAbort {
	fn from(e: SubsystemError) -> Self {
		UnexpectedAbort::SubsystemError(e)
	}
}

#[derive(Default, Clone)]
struct Shared(Arc<Mutex<SharedInner>>);

#[derive(Default)]
struct SharedInner {
	local_view: Option<View>,
	validation_peers: HashMap<PeerId, PeerData>,
	collation_peers: HashMap<PeerId, PeerData>,
}

enum Mode {
	Syncing(Box<dyn SyncOracle + Send>),
	Active,
}

async fn handle_subsystem_messages<Context, N, AD>(
	mut ctx: Context,
	mut network_service: N,
	mut authority_discovery_service: AD,
	shared: Shared,
	sync_oracle: Box<dyn SyncOracle + Send>,
	metrics: Metrics,
) -> Result<(), UnexpectedAbort>
where
	Context: SubsystemContext<Message = NetworkBridgeMessage>,
	N: Network,
	AD: validator_discovery::AuthorityDiscovery,
{
	// This is kept sorted, descending, by block number.
	let mut live_heads: Vec<ActivatedLeaf> = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut finalized_number = 0;
	let mut validator_discovery = validator_discovery::Service::<N, AD>::new();

	let mut mode = Mode::Syncing(sync_oracle);

	loop {
		futures::select! {
			msg = ctx.recv().fuse() => match msg {
				Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(active_leaves))) => {
					let ActiveLeavesUpdate { activated, deactivated } = active_leaves;
					tracing::trace!(
						target: LOG_TARGET,
						action = "ActiveLeaves",
						num_activated = %activated.len(),
						num_deactivated = %deactivated.len(),
					);

					for activated in activated {
						let pos = live_heads
							.binary_search_by(|probe| probe.number.cmp(&activated.number).reverse())
							.unwrap_or_else(|i| i);

						live_heads.insert(pos, activated);
					}
					live_heads.retain(|h| !deactivated.contains(&h.hash));

					// if we're done syncing, set the mode to `Mode::Active`.
					// Otherwise, we don't need to send view updates.
					{
						let is_done_syncing = match mode {
							Mode::Active => true,
							Mode::Syncing(ref mut sync_oracle) => !sync_oracle.is_major_syncing(),
						};

						if is_done_syncing {
							mode = Mode::Active;

							update_our_view(
								&mut network_service,
								&mut ctx,
								&live_heads,
								&shared,
								finalized_number,
								&metrics,
							).await?;
						}
					}
				}
				Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number))) => {
					tracing::trace!(
						target: LOG_TARGET,
						action = "BlockFinalized"
					);

					debug_assert!(finalized_number < number);

					// we don't send the view updates here, but delay them until the next `ActiveLeaves`
					// otherwise it might break assumptions of some of the subsystems
					// that we never send the same `ActiveLeavesUpdate`
					finalized_number = number;
				}
				Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => {
					return Ok(());
				}
				Ok(FromOverseer::Communication { msg }) => match msg {
					NetworkBridgeMessage::ReportPeer(peer, rep) => {
						if !rep.is_benefit() {
							tracing::debug!(
								target: LOG_TARGET,
								action = "ReportPeer"
							);
						}
						network_service.report_peer(peer, rep).await?
					}
					NetworkBridgeMessage::DisconnectPeer(peer, peer_set) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "DisconnectPeer",
							?peer,
							peer_set = ?peer_set,
						);
						network_service.disconnect_peer(peer, peer_set).await?;
					}
					NetworkBridgeMessage::SendValidationMessage(peers, msg) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "SendValidationMessages",
							num_messages = 1,
						);

						send_message(
							&mut network_service,
							peers,
							PeerSet::Validation,
							WireMessage::ProtocolMessage(msg),
							&metrics,
						).await?
					}
					NetworkBridgeMessage::SendValidationMessages(msgs) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "SendValidationMessages",
							num_messages = %msgs.len(),
						);

						for (peers, msg) in msgs {
							send_message(
								&mut network_service,
								peers,
								PeerSet::Validation,
								WireMessage::ProtocolMessage(msg),
								&metrics,
							).await?
						}
					}
					NetworkBridgeMessage::SendCollationMessage(peers, msg) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "SendCollationMessages",
							num_messages = 1,
						);

						send_message(
							&mut network_service,
							peers,
							PeerSet::Collation,
							WireMessage::ProtocolMessage(msg),
							&metrics,
						).await?
					}
					NetworkBridgeMessage::SendCollationMessages(msgs) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "SendCollationMessages",
							num_messages = %msgs.len(),
						);

						for (peers, msg) in msgs {
							send_message(
								&mut network_service,
								peers,
								PeerSet::Collation,
								WireMessage::ProtocolMessage(msg),
								&metrics,
							).await?
						}
					}
					NetworkBridgeMessage::SendRequests(reqs, if_disconnected) => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "SendRequests",
							num_requests = %reqs.len(),
						);

						for req in reqs {
							network_service
								.start_request(&mut authority_discovery_service, req, if_disconnected)
								.await;
						}
					}
					NetworkBridgeMessage::ConnectToValidators {
						validator_ids,
						peer_set,
						failed,
					} => {
						tracing::trace!(
							target: LOG_TARGET,
							action = "ConnectToValidators",
							peer_set = ?peer_set,
							ids = ?validator_ids,
							"Received a validator connection request",
						);

						metrics.note_desired_peer_count(peer_set, validator_ids.len());

						let (ns, ads) = validator_discovery.on_request(
							validator_ids,
							peer_set,
							failed,
							network_service,
							authority_discovery_service,
						).await;

						network_service = ns;
						authority_discovery_service = ads;
					}
				}
				Err(e) => return Err(e.into()),
			},
		}
	}
}

async fn handle_network_messages<AD: validator_discovery::AuthorityDiscovery>(
	mut sender: impl SubsystemSender,
	mut network_service: impl Network,
	mut network_stream: BoxStream<'static, NetworkEvent>,
	mut authority_discovery_service: AD,
	mut request_multiplexer: RequestMultiplexer,
	metrics: Metrics,
	shared: Shared,
) -> Result<(), UnexpectedAbort> {
	loop {
		futures::select! {
			network_event = network_stream.next().fuse() => match network_event {
				None => return Err(UnexpectedAbort::EventStreamConcluded),
				Some(NetworkEvent::Dht(_))
				| Some(NetworkEvent::SyncConnected { .. })
				| Some(NetworkEvent::SyncDisconnected { .. }) => {}
				Some(NetworkEvent::NotificationStreamOpened { remote: peer, protocol, role, .. }) => {
					let role = ObservedRole::from(role);
					let peer_set = match PeerSet::try_from_protocol_name(&protocol) {
						None => continue,
						Some(peer_set) => peer_set,
					};

					tracing::debug!(
						target: LOG_TARGET,
						action = "PeerConnected",
						peer_set = ?peer_set,
						peer = ?peer,
						role = ?role
					);

					let local_view = {
						let mut shared = shared.0.lock();
						let peer_map = match peer_set {
							PeerSet::Validation => &mut shared.validation_peers,
							PeerSet::Collation => &mut shared.collation_peers,
						};

						match peer_map.entry(peer.clone()) {
							hash_map::Entry::Occupied(_) => continue,
							hash_map::Entry::Vacant(vacant) => {
								vacant.insert(PeerData { view: View::default() });
							}
						}

						metrics.on_peer_connected(peer_set);
						metrics.note_peer_count(peer_set, peer_map.len());

						shared.local_view.clone().unwrap_or(View::default())
					};

					let maybe_authority =
						authority_discovery_service
							.get_authority_id_by_peer_id(peer).await;

					match peer_set {
						PeerSet::Validation => {
							dispatch_validation_events_to_all(
								vec![
									NetworkBridgeEvent::PeerConnected(peer.clone(), role, maybe_authority),
									NetworkBridgeEvent::PeerViewChange(
										peer.clone(),
										View::default(),
									),
								],
								&mut sender,
							).await;

							send_message(
								&mut network_service,
								vec![peer],
								PeerSet::Validation,
								WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
									local_view,
								),
								&metrics,
							).await?;
						}
						PeerSet::Collation => {
							dispatch_collation_events_to_all(
								vec![
									NetworkBridgeEvent::PeerConnected(peer.clone(), role, maybe_authority),
									NetworkBridgeEvent::PeerViewChange(
										peer.clone(),
										View::default(),
									),
								],
								&mut sender,
							).await;

							send_message(
								&mut network_service,
								vec![peer],
								PeerSet::Collation,
								WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(
									local_view,
								),
								&metrics,
							).await?;
						}
					}
				}
				Some(NetworkEvent::NotificationStreamClosed { remote: peer, protocol }) => {
					let peer_set = match PeerSet::try_from_protocol_name(&protocol) {
						None => continue,
						Some(peer_set) => peer_set,
					};

					tracing::debug!(
						target: LOG_TARGET,
						action = "PeerDisconnected",
						peer_set = ?peer_set,
						peer = ?peer
					);

					let was_connected = {
						let mut shared = shared.0.lock();
						let peer_map = match peer_set {
							PeerSet::Validation => &mut shared.validation_peers,
							PeerSet::Collation => &mut shared.collation_peers,
						};

						let w = peer_map.remove(&peer).is_some();

						metrics.on_peer_disconnected(peer_set);
						metrics.note_peer_count(peer_set, peer_map.len());

						w
					};

					if was_connected {
						match peer_set {
							PeerSet::Validation => dispatch_validation_event_to_all(
								NetworkBridgeEvent::PeerDisconnected(peer),
								&mut sender,
							).await,
							PeerSet::Collation => dispatch_collation_event_to_all(
								NetworkBridgeEvent::PeerDisconnected(peer),
								&mut sender,
							).await,
						}
					}
				}
				Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
					let v_messages: Result<Vec<_>, _> = messages
						.iter()
						.filter(|(protocol, _)| {
							protocol == &PeerSet::Validation.into_protocol_name()
						})
						.map(|(_, msg_bytes)| {
							WireMessage::decode(&mut msg_bytes.as_ref())
								.map(|m| (m, msg_bytes.len()))
						})
						.collect();

					let v_messages = match v_messages {
						Err(_) => {
							tracing::debug!(
								target: LOG_TARGET,
								action = "ReportPeer"
							);

							network_service.report_peer(remote, MALFORMED_MESSAGE_COST).await?;
							continue;
						}
						Ok(v) => v,
					};

					let c_messages: Result<Vec<_>, _> = messages
						.iter()
						.filter(|(protocol, _)| {
							protocol == &PeerSet::Collation.into_protocol_name()
						})
						.map(|(_, msg_bytes)| {
							WireMessage::decode(&mut msg_bytes.as_ref())
								.map(|m| (m, msg_bytes.len()))
						})
						.collect();

					match c_messages {
						Err(_) => {
							tracing::debug!(
								target: LOG_TARGET,
								action = "ReportPeer"
							);

							network_service.report_peer(remote, MALFORMED_MESSAGE_COST).await?;
							continue;
						}
						Ok(c_messages) => {
							if v_messages.is_empty() && c_messages.is_empty() {
								continue;
							} else {
								tracing::trace!(
									target: LOG_TARGET,
									action = "PeerMessages",
									peer = ?remote,
									num_validation_messages = %v_messages.len(),
									num_collation_messages = %c_messages.len()
								);

								if !v_messages.is_empty() {
									let (events, reports) = handle_peer_messages(
										remote.clone(),
										PeerSet::Validation,
										&mut shared.0.lock().validation_peers,
										v_messages,
										&metrics,
									);

									for report in reports {
										network_service.report_peer(remote.clone(), report).await?;
									}

									dispatch_validation_events_to_all(events, &mut sender).await;
								}

								if !c_messages.is_empty() {
									let (events, reports) = handle_peer_messages(
										remote.clone(),
										PeerSet::Collation,
										&mut shared.0.lock().collation_peers,
										c_messages,
										&metrics,
									);

									for report in reports {
										network_service.report_peer(remote.clone(), report).await?;
									}


									dispatch_collation_events_to_all(events, &mut sender).await;
								}
							}
						}
					}
				}
			},
			req_res_event = request_multiplexer.next().fuse() => match req_res_event {
				None => return Err(UnexpectedAbort::RequestStreamConcluded),
				Some(Err(err)) => {
					sender.send_message(NetworkBridgeMessage::ReportPeer(
						err.peer,
						MALFORMED_MESSAGE_COST,
					).into()).await;
				}
				Some(Ok(msg)) => {
					sender.send_message(msg).await;
				}
			},
		}
	}
}

/// Main driver, processing network events and messages from other subsystems.
///
/// THIS IS A HACK. We need to ensure we never hold the mutex across a `.await` boundary
/// and `parking_lot` currently does not provide `Send`, which helps us enforce that.
/// If this breaks, we need to find another way to protect ourselves.
///
/// ```compile_fail
/// #use parking_lot::MutexGuard;
/// #fn is_send<T: Send>();
/// #is_send::<parking_lot::MutexGuard<'static, ()>();
/// ```
#[tracing::instrument(skip(bridge, ctx, network_stream), fields(subsystem = LOG_TARGET))]
async fn run_network<N, AD>(
	bridge: NetworkBridge<N, AD>,
	mut ctx: impl SubsystemContext<Message=NetworkBridgeMessage>,
	network_stream: BoxStream<'static, NetworkEvent>,
) -> SubsystemResult<()>
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery,
{
	let shared = Shared::default();

	let NetworkBridge {
		network_service,
		mut request_multiplexer,
		authority_discovery_service,
		metrics,
		sync_oracle,
	 } = bridge;

	let statement_receiver = request_multiplexer
		.get_statement_fetching()
		.expect("Gets initialized, must be `Some` on startup. qed.");

	let (remote, network_event_handler) = handle_network_messages(
		ctx.sender().clone(),
		network_service.clone(),
		network_stream,
		authority_discovery_service.clone(),
		request_multiplexer,
		metrics.clone(),
		shared.clone(),
	).remote_handle();

	ctx.spawn("network-bridge-network-worker", Box::pin(remote)).await?;

	ctx.send_message(AllMessages::StatementDistribution(
		StatementDistributionMessage::StatementFetchingReceiver(statement_receiver)
	)).await;

	let subsystem_event_handler = handle_subsystem_messages(
		ctx,
		network_service,
		authority_discovery_service,
		shared,
		sync_oracle,
		metrics,
	);

	futures::pin_mut!(subsystem_event_handler);

	match futures::future::select(subsystem_event_handler, network_event_handler)
		.await
		.factor_first()
		.0
	{
		Ok(()) => Ok(()),
		Err(UnexpectedAbort::SubsystemError(err)) => {
			tracing::warn!(
				target: LOG_TARGET,
				err = ?err,
				"Shutting down Network Bridge due to error"
			);

			Err(SubsystemError::Context(format!(
				"Received SubsystemError from overseer: {:?}",
				err
			)))
		}
		Err(UnexpectedAbort::EventStreamConcluded) => {
			tracing::info!(
				target: LOG_TARGET,
				"Shutting down Network Bridge: underlying request stream concluded"
			);
			Err(SubsystemError::Context(
				"Incoming network event stream concluded.".to_string(),
			))
		}
		Err(UnexpectedAbort::RequestStreamConcluded) => {
			tracing::info!(
				target: LOG_TARGET,
				"Shutting down Network Bridge: underlying request stream concluded"
			);
			Err(SubsystemError::Context(
				"Incoming network request stream concluded".to_string(),
			))
		}
	}
}

fn construct_view(live_heads: impl DoubleEndedIterator<Item = Hash>, finalized_number: BlockNumber) -> View {
	View::new(
		live_heads.take(MAX_VIEW_HEADS),
		finalized_number,
	)
}

#[tracing::instrument(level = "trace", skip(net, ctx, shared, metrics), fields(subsystem = LOG_TARGET))]
async fn update_our_view(
	net: &mut impl Network,
	ctx: &mut impl SubsystemContext<Message = NetworkBridgeMessage>,
	live_heads: &[ActivatedLeaf],
	shared: &Shared,
	finalized_number: BlockNumber,
	metrics: &Metrics,
) -> SubsystemResult<()> {
	let new_view = construct_view(live_heads.iter().map(|v| v.hash), finalized_number);

	let (validation_peers, collation_peers) = {
		let mut shared = shared.0.lock();

		// We only want to send a view update when the heads changed.
		// A change in finalized block number only is _not_ sufficient.
		//
		// If this is the first view update since becoming active, but our view is empty,
		// there is no need to send anything.
		match shared.local_view {
			Some(ref v) if v.check_heads_eq(&new_view) => {
				return Ok(())
			}
			None if live_heads.is_empty() => {
				shared.local_view = Some(new_view);
				return Ok(())
			}
			_ => {
				shared.local_view = Some(new_view.clone());
			}

		}

		(
			shared.validation_peers.keys().cloned().collect::<Vec<_>>(),
			shared.collation_peers.keys().cloned().collect::<Vec<_>>(),
		)
	};

	send_validation_message(
		net,
		validation_peers,
		WireMessage::ViewUpdate(new_view.clone()),
		metrics,
	).await?;

	send_collation_message(
		net,
		collation_peers,
		WireMessage::ViewUpdate(new_view),
		metrics,
	).await?;

	let our_view = OurView::new(
		live_heads.iter().take(MAX_VIEW_HEADS).cloned().map(|a| (a.hash, a.span)),
		finalized_number,
	);

	dispatch_validation_event_to_all_unbounded(
		NetworkBridgeEvent::OurViewChange(our_view.clone()),
		ctx.sender(),
	);

	dispatch_collation_event_to_all_unbounded(
		NetworkBridgeEvent::OurViewChange(our_view),
		ctx.sender(),
	);

	Ok(())
}

// Handle messages on a specific peer-set. The peer is expected to be connected on that
// peer-set.
#[tracing::instrument(level = "trace", skip(peers, messages, metrics), fields(subsystem = LOG_TARGET))]
fn handle_peer_messages<M>(
	peer: PeerId,
	peer_set: PeerSet,
	peers: &mut HashMap<PeerId, PeerData>,
	messages: Vec<(WireMessage<M>, usize)>,
	metrics: &Metrics,
) -> (Vec<NetworkBridgeEvent<M>>, Vec<Rep>) {
	let peer_data = match peers.get_mut(&peer) {
		None => {
			return (Vec::new(), vec![UNCONNECTED_PEERSET_COST]);
		},
		Some(d) => d,
	};

	let mut outgoing_messages = Vec::with_capacity(messages.len());
	let mut reports = Vec::new();

	for (message, size_bytes) in messages {
		metrics.on_notification_received(peer_set, size_bytes);

		outgoing_messages.push(match message {
			WireMessage::ViewUpdate(new_view) => {
				if new_view.len() > MAX_VIEW_HEADS ||
					new_view.finalized_number < peer_data.view.finalized_number
				{
					reports.push(MALFORMED_VIEW_COST);
					continue
				} else if new_view.is_empty() {
					reports.push(EMPTY_VIEW_COST);
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

	(outgoing_messages, reports)
}

#[tracing::instrument(level = "trace", skip(net, peers, metrics), fields(subsystem = LOG_TARGET))]
async fn send_validation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::ValidationProtocol>,
	metrics: &Metrics,
) -> SubsystemResult<()>
	where
		I: IntoIterator<Item=PeerId>,
		I::IntoIter: ExactSizeIterator,
{
	send_message(net, peers, PeerSet::Validation, message, metrics).await
}

#[tracing::instrument(level = "trace", skip(net, peers, metrics), fields(subsystem = LOG_TARGET))]
async fn send_collation_message<I>(
	net: &mut impl Network,
	peers: I,
	message: WireMessage<protocol_v1::CollationProtocol>,
	metrics: &Metrics,
) -> SubsystemResult<()>
	where
	I: IntoIterator<Item=PeerId>,
	I::IntoIter: ExactSizeIterator,
{
	send_message(net, peers, PeerSet::Collation, message, metrics).await
}


async fn dispatch_validation_event_to_all(
	event: NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
	ctx: &mut impl SubsystemSender
) {
	dispatch_validation_events_to_all(std::iter::once(event), ctx).await
}

async fn dispatch_collation_event_to_all(
	event: NetworkBridgeEvent<protocol_v1::CollationProtocol>,
	ctx: &mut impl SubsystemSender
) {
	dispatch_collation_events_to_all(std::iter::once(event), ctx).await
}

fn dispatch_validation_event_to_all_unbounded(
	event: NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
	ctx: &mut impl SubsystemSender
) {
	for msg in AllMessages::dispatch_iter(event) {
		ctx.send_unbounded_message(msg);
	}
}

fn dispatch_collation_event_to_all_unbounded(
	event: NetworkBridgeEvent<protocol_v1::CollationProtocol>,
	ctx: &mut impl SubsystemSender
) {
	if let Some(msg) = event.focus().ok().map(CollatorProtocolMessage::NetworkBridgeUpdateV1) {
		ctx.send_unbounded_message(msg.into());
	}
}

#[tracing::instrument(level = "trace", skip(events, ctx), fields(subsystem = LOG_TARGET))]
async fn dispatch_validation_events_to_all<I>(
	events: I,
	ctx: &mut impl SubsystemSender
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
	ctx: &mut impl SubsystemSender
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
	use futures::channel::oneshot;

	use std::borrow::Cow;
	use std::collections::HashSet;
	use std::pin::Pin;
	use std::sync::atomic::{AtomicBool, Ordering};
	use async_trait::async_trait;
	use parking_lot::Mutex;
	use assert_matches::assert_matches;

	use sc_network::{Event as NetworkEvent, IfDisconnected};

	use polkadot_subsystem::{jaeger, ActiveLeavesUpdate, FromOverseer, OverseerSignal};
	use polkadot_subsystem::messages::{
		ApprovalDistributionMessage,
		BitfieldDistributionMessage,
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
	#[derive(Clone)]
	struct TestNetwork {
		net_events: Arc<Mutex<Option<SingleItemStream<NetworkEvent>>>>,
		action_tx: metered::UnboundedMeteredSender<NetworkAction>,
		_req_configs: Vec<RequestResponseConfig>,
	}

	#[derive(Clone)]
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
		let (action_tx, action_rx) = metered::unbounded();

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

		async fn add_to_peers_set(&mut self, _protocol: Cow<'static, str>, _: HashSet<Multiaddr>) -> Result<(), String> {
			Ok(())
		}

		async fn remove_from_peers_set(&mut self, _protocol: Cow<'static, str>, _: HashSet<Multiaddr>) -> Result<(), String> {
			Ok(())
		}

		fn action_sink<'a>(&'a mut self)
			-> Pin<Box<dyn Sink<NetworkAction, Error = SubsystemError> + Send + 'a>>
		{
			Box::pin((&mut self.action_tx).sink_map_err(Into::into))
		}

		async fn start_request<AD: AuthorityDiscovery>(&self, _: &mut AD, _: Requests, _: IfDisconnected) {
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
				negotiated_fallback: None,
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

	#[derive(Clone)]
	struct TestSyncOracle {
		flag: Arc<AtomicBool>,
		done_syncing_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
	}

	struct TestSyncOracleHandle {
		done_syncing_receiver: oneshot::Receiver<()>,
		flag: Arc<AtomicBool>,
	}

	impl TestSyncOracleHandle {
		fn set_done(&self) {
			self.flag.store(false, Ordering::SeqCst);
		}

		async fn await_mode_switch(self) {
			let _ = self.done_syncing_receiver.await;
		}
	}

	impl SyncOracle for TestSyncOracle {
		fn is_major_syncing(&mut self) -> bool {
			let is_major_syncing = self.flag.load(Ordering::SeqCst);

			if !is_major_syncing {
				if let Some(sender) = self.done_syncing_sender.lock().take() {
					let _ = sender.send(());
				}
			}

			is_major_syncing
		}

		fn is_offline(&mut self) -> bool {
			unimplemented!("not used in network bridge")
		}
	}

	// val - result of `is_major_syncing`.
	fn make_sync_oracle(val: bool) -> (TestSyncOracle, TestSyncOracleHandle) {
		let (tx, rx) = oneshot::channel();
		let flag = Arc::new(AtomicBool::new(val));

		(
			TestSyncOracle {
				flag: flag.clone(),
				done_syncing_sender: Arc::new(Mutex::new(Some(tx))),
			},
			TestSyncOracleHandle {
				flag,
				done_syncing_receiver: rx,
			}
		)
	}

	fn done_syncing_oracle() -> Box<dyn SyncOracle + Send> {
		let (oracle, _) = make_sync_oracle(false);
		Box::new(oracle)
	}

	type VirtualOverseer = TestSubsystemContextHandle<NetworkBridgeMessage>;

	struct TestHarness {
		network_handle: TestNetworkHandle,
		virtual_overseer: VirtualOverseer,
	}

	fn test_harness<T: Future<Output=VirtualOverseer>>(
		sync_oracle: Box<dyn SyncOracle + Send>,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let pool = sp_core::testing::TaskExecutor::new();
		let (request_multiplexer, req_configs) = RequestMultiplexer::new();
		let (mut network, network_handle, discovery) = new_test_network(req_configs);
		let (context, virtual_overseer) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
		let network_stream = network.event_stream();

		let bridge = NetworkBridge {
			network_service: network,
			authority_discovery_service: discovery,
			request_multiplexer,
			metrics: Metrics(None),
			sync_oracle,
		};

		let network_bridge = run_network(
			bridge,
			context,
			network_stream,
		)
			.map_err(|_| panic!("subsystem execution failed"))
			.map(|_| ());

		let test_fut = test(TestHarness {
			network_handle,
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(network_bridge);

		let _ = executor::block_on(future::join(async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}, network_bridge));
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
			AllMessages::BitfieldDistribution(
				BitfieldDistributionMessage::NetworkBridgeUpdateV1(e)
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
		let (oracle, handle) = make_sync_oracle(false);
		test_harness(Box::new(oracle), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			let head = Hash::repeat_byte(1);
			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: head,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
				))
			).await;

			handle.await_mode_switch().await;

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
			virtual_overseer
		});
	}

	#[test]
	fn sends_view_updates_to_peers() {
		let (oracle, handle) = make_sync_oracle(false);
		test_harness(Box::new(oracle), |test_harness| async move {
			let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate {
						activated: Default::default(),
						deactivated: Default::default(),
					}
				))
			).await;

			handle.await_mode_switch().await;

			network_handle.connect_peer(
				peer_a.clone(),
				PeerSet::Validation,
				ObservedRole::Full,
			).await;
			network_handle.connect_peer(
				peer_b.clone(),
				PeerSet::Collation,
				ObservedRole::Full,
			).await;

			let actions = network_handle.next_network_actions(2).await;
			let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
				View::default(),
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
					PeerSet::Collation,
					wire_message.clone(),
				),
			);

			let hash_a = Hash::repeat_byte(1);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_a,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
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
					PeerSet::Collation,
					wire_message.clone(),
				),
			);
			virtual_overseer
		});
	}

	#[test]
	fn do_not_send_view_update_until_synced() {
		let (oracle, handle) = make_sync_oracle(true);
		test_harness(Box::new(oracle), |test_harness| async move {
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
				PeerSet::Collation,
				ObservedRole::Full,
			).await;

			{
				let actions = network_handle.next_network_actions(2).await;
				let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
					View::default(),
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
						PeerSet::Collation,
						wire_message.clone(),
					),
				);
			}

			let hash_a = Hash::repeat_byte(1);
			let hash_b = Hash::repeat_byte(1);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_a,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
				))
			).await;

			// delay until the previous update has certainly been processed.
			futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;

			handle.set_done();

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_b,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
				))
			).await;

			handle.await_mode_switch().await;

			// There should be a mode switch only for the second view update.
			{
				let actions = network_handle.next_network_actions(2).await;
				let wire_message = WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(
					view![hash_a, hash_b]
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
						PeerSet::Collation,
						wire_message.clone(),
					),
				);
			}
			virtual_overseer
		});
	}

	#[test]
	fn do_not_send_view_update_when_only_finalized_block_changed() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
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
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_a,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
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
			virtual_overseer
		});
	}

	#[test]
	fn peer_view_updates_sent_via_overseer() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;

			let view = view![Hash::repeat_byte(1)];

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
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
			virtual_overseer
		});
	}

	#[test]
	fn peer_messages_sent_via_overseer() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
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

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			let approval_distribution_message = protocol_v1::ApprovalDistributionMessage::Approvals(
				Vec::new()
			);

			let message = protocol_v1::ValidationProtocol::ApprovalDistribution(
				approval_distribution_message.clone(),
			);

			network_handle.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::ProtocolMessage(message.clone()).encode(),
			).await;

			network_handle.disconnect_peer(peer.clone(), PeerSet::Validation).await;

			// Approval distribution message comes first, and the message is only sent to that subsystem.
			// then a disconnection event arises that is sent to all validation networking subsystems.

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::ApprovalDistribution(
					ApprovalDistributionMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerMessage(p, m)
					)
				) => {
					assert_eq!(p, peer);
					assert_eq!(m, approval_distribution_message);
				}
			);

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerDisconnected(peer),
				&mut virtual_overseer,
			).await;
			virtual_overseer
		});
	}

	#[test]
	fn peer_disconnect_from_just_one_peerset() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
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
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_a,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
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
			virtual_overseer
		});
	}

	#[test]
	fn relays_collation_protocol_messages() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			network_handle.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer_b.clone(), PeerSet::Collation, ObservedRole::Full).await;

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer_a.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			// peer A gets reported for sending a collation message.

			let collator_protocol_message = protocol_v1::CollatorProtocolMessage::Declare(
				Sr25519Keyring::Alice.public().into(),
				Default::default(),
				Default::default(),
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
			virtual_overseer
		});
	}

	#[test]
	fn different_views_on_different_peer_sets() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
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
			virtual_overseer
		});
	}

	#[test]
	fn sent_views_include_finalized_number_update() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
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
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: hash_b,
						number: 1,
						span: Arc::new(jaeger::Span::Disabled),
					})
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
			virtual_overseer
		});
	}

	#[test]
	fn view_finalized_number_can_not_go_down() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness { mut network_handle, virtual_overseer } = test_harness;

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
			virtual_overseer
		});
	}

	#[test]
	fn send_messages_to_peers() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut network_handle,
				mut virtual_overseer,
			} = test_harness;

			let peer = PeerId::random();

			network_handle.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full).await;
			network_handle.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			// bridge will inform about all connected peers.
			{
				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
					&mut virtual_overseer,
				).await;

				assert_sends_validation_event_to_all(
					NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
					&mut virtual_overseer,
				).await;
			}

			{
				assert_sends_collation_event_to_all(
					NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full, None),
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
				let approval_distribution_message = protocol_v1::ApprovalDistributionMessage::Approvals(
					Vec::new()
				);

				let message = protocol_v1::ValidationProtocol::ApprovalDistribution(
					approval_distribution_message.clone(),
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
					Sr25519Keyring::Alice.public().into(),
					Default::default(),
					Default::default(),
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
			virtual_overseer
		});
	}

	#[test]
	fn spread_event_to_subsystems_is_up_to_date() {
		// Number of subsystems expected to be interested in a network event,
		// and hence the network event broadcasted to.
		const EXPECTED_COUNT: usize = 3;

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
				AllMessages::AvailabilityRecovery(_) => unreachable!("Not interested in network events"),
				AllMessages::BitfieldDistribution(_) => { cnt += 1; }
				AllMessages::BitfieldSigning(_) => unreachable!("Not interested in network events"),
				AllMessages::Provisioner(_) => unreachable!("Not interested in network events"),
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

	#[test]
	fn our_view_updates_decreasing_order_and_limited_to_max() {
		test_harness(done_syncing_oracle(), |test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
				..
			} = test_harness;


			// to show that we're still connected on the collation protocol, send a view update.

			let hashes = (0..MAX_VIEW_HEADS * 3).map(|i| Hash::repeat_byte(i as u8));

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					// These are in reverse order, so the subsystem must sort internally to
					// get the correct view.
					ActiveLeavesUpdate {
						activated: hashes.enumerate().map(|(i, h)| ActivatedLeaf {
							hash: h,
							number: i as _,
							span: Arc::new(jaeger::Span::Disabled),
						}).rev().collect(),
						deactivated: Default::default(),
					}
				))
			).await;

			let view_heads = (MAX_VIEW_HEADS * 2 .. MAX_VIEW_HEADS * 3).rev()
				.map(|i| (Hash::repeat_byte(i as u8), Arc::new(jaeger::Span::Disabled)) );

			let our_view = OurView::new(
				view_heads,
				0,
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::StatementFetchingReceiver(_)
				)
			);

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::OurViewChange(our_view.clone()),
				&mut virtual_overseer,
			).await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::OurViewChange(our_view),
				&mut virtual_overseer,
			).await;
			virtual_overseer
		});
	}
}
