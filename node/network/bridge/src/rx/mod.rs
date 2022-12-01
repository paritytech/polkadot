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

//! The Network Bridge Subsystem - handles _incoming_ messages from the network, forwarded to the relevant subsystems.
use super::*;

use always_assert::never;
use bytes::Bytes;
use futures::stream::BoxStream;
use parity_scale_codec::{Decode, DecodeAll};

use sc_network::Event as NetworkEvent;
use sp_consensus::SyncOracle;

use polkadot_node_network_protocol::{
	self as net_protocol,
	grid_topology::{SessionGridTopology, TopologyPeerInfo},
	peer_set::{
		CollationVersion, PeerSet, PeerSetProtocolNames, PerPeerSet, ProtocolVersion,
		ValidationVersion,
	},
	v1 as protocol_v1, ObservedRole, OurView, PeerId, UnifiedReputationChange as Rep, View,
};

use polkadot_node_subsystem::{
	errors::SubsystemError,
	messages::{
		network_bridge_event::NewGossipTopology, ApprovalDistributionMessage,
		BitfieldDistributionMessage, CollatorProtocolMessage, GossipSupportMessage,
		NetworkBridgeEvent, NetworkBridgeRxMessage, StatementDistributionMessage,
	},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem,
};

use polkadot_primitives::v2::{AuthorityDiscoveryId, BlockNumber, Hash, ValidatorIndex};

/// Peer set info for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::{peer_sets_info, IsAuthority};

use std::{
	collections::{hash_map, HashMap},
	iter::ExactSizeIterator,
};

use super::validator_discovery;

/// Actual interfacing to the network based on the `Network` trait.
///
/// Defines the `Network` trait with an implementation for an `Arc<NetworkService>`.
use crate::network::{send_message, Network};

use crate::network::get_peer_id_by_authority_id;

use super::metrics::Metrics;

#[cfg(test)]
mod tests;

// network bridge log target
const LOG_TARGET: &'static str = "parachain::network-bridge-rx";

/// The network bridge subsystem - network receiving side.
pub struct NetworkBridgeRx<N, AD> {
	/// `Network` trait implementing type.
	network_service: N,
	authority_discovery_service: AD,
	sync_oracle: Box<dyn SyncOracle + Send>,
	shared: Shared,
	metrics: Metrics,
	peerset_protocol_names: PeerSetProtocolNames,
}

impl<N, AD> NetworkBridgeRx<N, AD> {
	/// Create a new network bridge subsystem with underlying network service and authority discovery service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`peers_sets_info`](peers_sets_info).
	pub fn new(
		network_service: N,
		authority_discovery_service: AD,
		sync_oracle: Box<dyn SyncOracle + Send>,
		metrics: Metrics,
		peerset_protocol_names: PeerSetProtocolNames,
	) -> Self {
		let shared = Shared::default();
		Self {
			network_service,
			authority_discovery_service,
			sync_oracle,
			shared,
			metrics,
			peerset_protocol_names,
		}
	}
}

#[overseer::subsystem(NetworkBridgeRx, error = SubsystemError, prefix = self::overseer)]
impl<Net, AD, Context> NetworkBridgeRx<Net, AD>
where
	Net: Network + Sync,
	AD: validator_discovery::AuthorityDiscovery + Clone + Sync,
{
	fn start(mut self, ctx: Context) -> SpawnedSubsystem {
		// The stream of networking events has to be created at initialization, otherwise the
		// networking might open connections before the stream of events has been grabbed.
		let network_stream = self.network_service.event_stream();

		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		let future = run_network_in(self, ctx, network_stream)
			.map_err(|e| SubsystemError::with_origin("network-bridge", e))
			.boxed();
		SpawnedSubsystem { name: "network-bridge-rx-subsystem", future }
	}
}

async fn handle_network_messages<AD>(
	mut sender: impl overseer::NetworkBridgeRxSenderTrait,
	mut network_service: impl Network,
	network_stream: BoxStream<'static, NetworkEvent>,
	mut authority_discovery_service: AD,
	metrics: Metrics,
	shared: Shared,
	peerset_protocol_names: PeerSetProtocolNames,
) -> Result<(), Error>
where
	AD: validator_discovery::AuthorityDiscovery + Send,
{
	let mut network_stream = network_stream.fuse();
	loop {
		match network_stream.next().await {
			None => return Err(Error::EventStreamConcluded),
			Some(NetworkEvent::Dht(_)) |
			Some(NetworkEvent::SyncConnected { .. }) |
			Some(NetworkEvent::SyncDisconnected { .. }) => {},
			Some(NetworkEvent::NotificationStreamOpened {
				remote: peer,
				protocol,
				role,
				negotiated_fallback,
			}) => {
				let role = ObservedRole::from(role);
				let (peer_set, version) = {
					let (peer_set, version) =
						match peerset_protocol_names.try_get_protocol(&protocol) {
							None => continue,
							Some(p) => p,
						};

					if let Some(fallback) = negotiated_fallback {
						match peerset_protocol_names.try_get_protocol(&fallback) {
							None => {
								gum::debug!(
									target: LOG_TARGET,
									fallback = &*fallback,
									?peer,
									?peer_set,
									"Unknown fallback",
								);

								continue
							},
							Some((p2, v2)) => {
								if p2 != peer_set {
									gum::debug!(
										target: LOG_TARGET,
										fallback = &*fallback,
										fallback_peerset = ?p2,
										protocol = &*protocol,
										peerset = ?peer_set,
										"Fallback mismatched peer-set",
									);

									continue
								}

								(p2, v2)
							},
						}
					} else {
						(peer_set, version)
					}
				};

				gum::debug!(
					target: LOG_TARGET,
					action = "PeerConnected",
					peer_set = ?peer_set,
					version = %version,
					peer = ?peer,
					role = ?role
				);

				let local_view = {
					let mut shared = shared.0.lock();
					let peer_map = match peer_set {
						PeerSet::Validation => &mut shared.validation_peers,
						PeerSet::Collation => &mut shared.collation_peers,
					};

					match peer_map.entry(peer) {
						hash_map::Entry::Occupied(_) => continue,
						hash_map::Entry::Vacant(vacant) => {
							vacant.insert(PeerData { view: View::default(), version });
						},
					}

					metrics.on_peer_connected(peer_set, version);
					metrics.note_peer_count(peer_set, version, peer_map.len());

					shared.local_view.clone().unwrap_or(View::default())
				};

				let maybe_authority =
					authority_discovery_service.get_authority_ids_by_peer_id(peer).await;

				match peer_set {
					PeerSet::Validation => {
						dispatch_validation_events_to_all(
							vec![
								NetworkBridgeEvent::PeerConnected(
									peer,
									role,
									version,
									maybe_authority,
								),
								NetworkBridgeEvent::PeerViewChange(peer, View::default()),
							],
							&mut sender,
						)
						.await;

						send_message(
							&mut network_service,
							vec![peer],
							PeerSet::Validation,
							version,
							&peerset_protocol_names,
							WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(local_view),
							&metrics,
						);
					},
					PeerSet::Collation => {
						dispatch_collation_events_to_all(
							vec![
								NetworkBridgeEvent::PeerConnected(
									peer,
									role,
									version,
									maybe_authority,
								),
								NetworkBridgeEvent::PeerViewChange(peer, View::default()),
							],
							&mut sender,
						)
						.await;

						send_message(
							&mut network_service,
							vec![peer],
							PeerSet::Collation,
							version,
							&peerset_protocol_names,
							WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(local_view),
							&metrics,
						);
					},
				}
			},
			Some(NetworkEvent::NotificationStreamClosed { remote: peer, protocol }) => {
				let (peer_set, version) = match peerset_protocol_names.try_get_protocol(&protocol) {
					None => continue,
					Some(peer_set) => peer_set,
				};

				gum::debug!(
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

					metrics.on_peer_disconnected(peer_set, version);
					metrics.note_peer_count(peer_set, version, peer_map.len());

					w
				};

				if was_connected && version == peer_set.get_main_version() {
					match peer_set {
						PeerSet::Validation =>
							dispatch_validation_event_to_all(
								NetworkBridgeEvent::PeerDisconnected(peer),
								&mut sender,
							)
							.await,
						PeerSet::Collation =>
							dispatch_collation_event_to_all(
								NetworkBridgeEvent::PeerDisconnected(peer),
								&mut sender,
							)
							.await,
					}
				}
			},
			Some(NetworkEvent::NotificationsReceived { remote, messages }) => {
				let expected_versions = {
					let mut versions = PerPeerSet::<Option<ProtocolVersion>>::default();
					let shared = shared.0.lock();
					if let Some(peer_data) = shared.validation_peers.get(&remote) {
						versions[PeerSet::Validation] = Some(peer_data.version);
					}

					if let Some(peer_data) = shared.collation_peers.get(&remote) {
						versions[PeerSet::Collation] = Some(peer_data.version);
					}

					versions
				};

				// non-decoded, but version-checked validation messages.
				let v_messages: Result<Vec<_>, _> = messages
					.iter()
					.filter_map(|(protocol, msg_bytes)| {
						// version doesn't matter because we always receive on the 'correct'
						// protocol name, not the negotiated fallback.
						let (peer_set, _version) =
							peerset_protocol_names.try_get_protocol(protocol)?;
						if peer_set == PeerSet::Validation {
							if expected_versions[PeerSet::Validation].is_none() {
								return Some(Err(UNCONNECTED_PEERSET_COST))
							}

							Some(Ok(msg_bytes.clone()))
						} else {
							None
						}
					})
					.collect();

				let v_messages = match v_messages {
					Err(rep) => {
						gum::debug!(target: LOG_TARGET, action = "ReportPeer");
						network_service.report_peer(remote, rep);

						continue
					},
					Ok(v) => v,
				};

				// non-decoded, but version-checked colldation messages.
				let c_messages: Result<Vec<_>, _> = messages
					.iter()
					.filter_map(|(protocol, msg_bytes)| {
						// version doesn't matter because we always receive on the 'correct'
						// protocol name, not the negotiated fallback.
						let (peer_set, _version) =
							peerset_protocol_names.try_get_protocol(protocol)?;

						if peer_set == PeerSet::Collation {
							if expected_versions[PeerSet::Collation].is_none() {
								return Some(Err(UNCONNECTED_PEERSET_COST))
							}

							Some(Ok(msg_bytes.clone()))
						} else {
							None
						}
					})
					.collect();

				let c_messages = match c_messages {
					Err(rep) => {
						gum::debug!(target: LOG_TARGET, action = "ReportPeer");
						network_service.report_peer(remote, rep);

						continue
					},
					Ok(v) => v,
				};

				if v_messages.is_empty() && c_messages.is_empty() {
					continue
				}

				gum::trace!(
					target: LOG_TARGET,
					action = "PeerMessages",
					peer = ?remote,
					num_validation_messages = %v_messages.len(),
					num_collation_messages = %c_messages.len()
				);

				if !v_messages.is_empty() {
					let (events, reports) =
						if expected_versions[PeerSet::Validation] ==
							Some(ValidationVersion::V1.into())
						{
							handle_v1_peer_messages::<protocol_v1::ValidationProtocol, _>(
								remote,
								PeerSet::Validation,
								&mut shared.0.lock().validation_peers,
								v_messages,
								&metrics,
							)
						} else {
							gum::warn!(
								target: LOG_TARGET,
								version = ?expected_versions[PeerSet::Validation],
								"Major logic bug. Peer somehow has unsupported validation protocol version."
							);

							never!("Only version 1 is supported; peer set connection checked above; qed");

							// If a peer somehow triggers this, we'll disconnect them
							// eventually.
							(Vec::new(), vec![UNCONNECTED_PEERSET_COST])
						};

					for report in reports {
						network_service.report_peer(remote, report);
					}

					dispatch_validation_events_to_all(events, &mut sender).await;
				}

				if !c_messages.is_empty() {
					let (events, reports) =
						if expected_versions[PeerSet::Collation] ==
							Some(CollationVersion::V1.into())
						{
							handle_v1_peer_messages::<protocol_v1::CollationProtocol, _>(
								remote,
								PeerSet::Collation,
								&mut shared.0.lock().collation_peers,
								c_messages,
								&metrics,
							)
						} else {
							gum::warn!(
								target: LOG_TARGET,
								version = ?expected_versions[PeerSet::Collation],
								"Major logic bug. Peer somehow has unsupported collation protocol version."
							);

							never!("Only version 1 is supported; peer set connection checked above; qed");

							// If a peer somehow triggers this, we'll disconnect them
							// eventually.
							(Vec::new(), vec![UNCONNECTED_PEERSET_COST])
						};

					for report in reports {
						network_service.report_peer(remote, report);
					}

					dispatch_collation_events_to_all(events, &mut sender).await;
				}
			},
		}
	}
}

async fn flesh_out_topology_peers<AD, N>(ads: &mut AD, neighbors: N) -> Vec<TopologyPeerInfo>
where
	AD: validator_discovery::AuthorityDiscovery,
	N: IntoIterator<Item = (AuthorityDiscoveryId, ValidatorIndex)>,
	N::IntoIter: std::iter::ExactSizeIterator,
{
	let neighbors = neighbors.into_iter();
	let mut peers = Vec::with_capacity(neighbors.len());
	for (discovery_id, validator_index) in neighbors {
		let addr = get_peer_id_by_authority_id(ads, discovery_id.clone()).await;
		peers.push(TopologyPeerInfo {
			peer_ids: addr.into_iter().collect(),
			validator_index,
			discovery_id,
		});
	}

	peers
}

#[overseer::contextbounds(NetworkBridgeRx, prefix = self::overseer)]
async fn run_incoming_orchestra_signals<Context, N, AD>(
	mut ctx: Context,
	mut network_service: N,
	mut authority_discovery_service: AD,
	shared: Shared,
	sync_oracle: Box<dyn SyncOracle + Send>,
	metrics: Metrics,
	peerset_protocol_names: PeerSetProtocolNames,
) -> Result<(), Error>
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery + Clone,
{
	// This is kept sorted, descending, by block number.
	let mut live_heads: Vec<ActivatedLeaf> = Vec::with_capacity(MAX_VIEW_HEADS);
	let mut finalized_number = 0;

	let mut mode = Mode::Syncing(sync_oracle);
	loop {
		match ctx.recv().fuse().await? {
			FromOrchestra::Communication {
				msg:
					NetworkBridgeRxMessage::NewGossipTopology {
						session,
						local_index,
						canonical_shuffling,
						shuffled_indices,
					},
			} => {
				gum::debug!(
					target: LOG_TARGET,
					action = "NewGossipTopology",
					?session,
					?local_index,
					"Gossip topology has changed",
				);

				let topology_peers =
					flesh_out_topology_peers(&mut authority_discovery_service, canonical_shuffling)
						.await;

				dispatch_validation_event_to_all_unbounded(
					NetworkBridgeEvent::NewGossipTopology(NewGossipTopology {
						session,
						topology: SessionGridTopology::new(shuffled_indices, topology_peers),
						local_index,
					}),
					ctx.sender(),
				);
			},
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(active_leaves)) => {
				let ActiveLeavesUpdate { activated, deactivated } = active_leaves;
				gum::trace!(
					target: LOG_TARGET,
					action = "ActiveLeaves",
					has_activated = activated.is_some(),
					num_deactivated = %deactivated.len(),
				);

				if let Some(activated) = activated {
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
							&peerset_protocol_names,
						);
					}
				}
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(_hash, number)) => {
				gum::trace!(target: LOG_TARGET, action = "BlockFinalized");

				debug_assert!(finalized_number < number);

				// we don't send the view updates here, but delay them until the next `ActiveLeaves`
				// otherwise it might break assumptions of some of the subsystems
				// that we never send the same `ActiveLeavesUpdate`
				finalized_number = number;
			},
		}
	}
}

/// Main driver, processing network events and overseer signals.
///
/// THIS IS A HACK. We need to ensure we never hold the mutex across an `.await` boundary
/// and `parking_lot` currently does not provide `Send`, which helps us enforce that.
/// If this breaks, we need to find another way to protect ourselves.
///
/// ```compile_fail
/// #use parking_lot::MutexGuard;
/// #fn is_send<T: Send>();
/// #is_send::<parking_lot::MutexGuard<'static, ()>();
/// ```
#[overseer::contextbounds(NetworkBridgeRx, prefix = self::overseer)]
async fn run_network_in<N, AD, Context>(
	bridge: NetworkBridgeRx<N, AD>,
	mut ctx: Context,
	network_stream: BoxStream<'static, NetworkEvent>,
) -> Result<(), Error>
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery + Clone,
{
	let NetworkBridgeRx {
		network_service,
		authority_discovery_service,
		metrics,
		sync_oracle,
		shared,
		peerset_protocol_names,
	} = bridge;

	let (task, network_event_handler) = handle_network_messages(
		ctx.sender().clone(),
		network_service.clone(),
		network_stream,
		authority_discovery_service.clone(),
		metrics.clone(),
		shared.clone(),
		peerset_protocol_names.clone(),
	)
	.remote_handle();

	ctx.spawn("network-bridge-in-network-worker", Box::pin(task))?;
	futures::pin_mut!(network_event_handler);

	let orchestra_signal_handler = run_incoming_orchestra_signals(
		ctx,
		network_service,
		authority_discovery_service,
		shared,
		sync_oracle,
		metrics,
		peerset_protocol_names,
	);

	futures::pin_mut!(orchestra_signal_handler);

	futures::future::select(orchestra_signal_handler, network_event_handler)
		.await
		.factor_first()
		.0?;
	Ok(())
}

fn construct_view(
	live_heads: impl DoubleEndedIterator<Item = Hash>,
	finalized_number: BlockNumber,
) -> View {
	View::new(live_heads.take(MAX_VIEW_HEADS), finalized_number)
}

#[overseer::contextbounds(NetworkBridgeRx, prefix = self::overseer)]
fn update_our_view<Net, Context>(
	net: &mut Net,
	ctx: &mut Context,
	live_heads: &[ActivatedLeaf],
	shared: &Shared,
	finalized_number: BlockNumber,
	metrics: &Metrics,
	peerset_protocol_names: &PeerSetProtocolNames,
) where
	Net: Network,
{
	let new_view = construct_view(live_heads.iter().map(|v| v.hash), finalized_number);

	let (validation_peers, collation_peers) = {
		let mut shared = shared.0.lock();

		// We only want to send a view update when the heads changed.
		// A change in finalized block number only is _not_ sufficient.
		//
		// If this is the first view update since becoming active, but our view is empty,
		// there is no need to send anything.
		match shared.local_view {
			Some(ref v) if v.check_heads_eq(&new_view) => return,
			None if live_heads.is_empty() => {
				shared.local_view = Some(new_view);
				return
			},
			_ => {
				shared.local_view = Some(new_view.clone());
			},
		}

		(
			shared.validation_peers.keys().cloned().collect::<Vec<_>>(),
			shared.collation_peers.keys().cloned().collect::<Vec<_>>(),
		)
	};

	send_validation_message_v1(
		net,
		validation_peers,
		peerset_protocol_names,
		WireMessage::ViewUpdate(new_view.clone()),
		metrics,
	);

	send_collation_message_v1(
		net,
		collation_peers,
		peerset_protocol_names,
		WireMessage::ViewUpdate(new_view),
		metrics,
	);

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
}

// Handle messages on a specific v1 peer-set. The peer is expected to be connected on that
// peer-set.
fn handle_v1_peer_messages<RawMessage: Decode, OutMessage: From<RawMessage>>(
	peer: PeerId,
	peer_set: PeerSet,
	peers: &mut HashMap<PeerId, PeerData>,
	messages: Vec<Bytes>,
	metrics: &Metrics,
) -> (Vec<NetworkBridgeEvent<OutMessage>>, Vec<Rep>) {
	let peer_data = match peers.get_mut(&peer) {
		None => return (Vec::new(), vec![UNCONNECTED_PEERSET_COST]),
		Some(d) => d,
	};

	let mut outgoing_events = Vec::with_capacity(messages.len());
	let mut reports = Vec::new();

	for message in messages {
		metrics.on_notification_received(peer_set, peer_data.version, message.len());
		let message = match WireMessage::<RawMessage>::decode_all(&mut message.as_ref()) {
			Err(_) => {
				reports.push(MALFORMED_MESSAGE_COST);
				continue
			},
			Ok(m) => m,
		};

		outgoing_events.push(match message {
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

					NetworkBridgeEvent::PeerViewChange(peer, peer_data.view.clone())
				}
			},
			WireMessage::ProtocolMessage(message) =>
				NetworkBridgeEvent::PeerMessage(peer, message.into()),
		})
	}

	(outgoing_events, reports)
}

fn send_validation_message_v1(
	net: &mut impl Network,
	peers: Vec<PeerId>,
	peerset_protocol_names: &PeerSetProtocolNames,
	message: WireMessage<protocol_v1::ValidationProtocol>,
	metrics: &Metrics,
) {
	send_message(
		net,
		peers,
		PeerSet::Validation,
		ValidationVersion::V1.into(),
		peerset_protocol_names,
		message,
		metrics,
	);
}

fn send_collation_message_v1(
	net: &mut impl Network,
	peers: Vec<PeerId>,
	peerset_protocol_names: &PeerSetProtocolNames,
	message: WireMessage<protocol_v1::CollationProtocol>,
	metrics: &Metrics,
) {
	send_message(
		net,
		peers,
		PeerSet::Collation,
		CollationVersion::V1.into(),
		peerset_protocol_names,
		message,
		metrics,
	);
}

async fn dispatch_validation_event_to_all(
	event: NetworkBridgeEvent<net_protocol::VersionedValidationProtocol>,
	ctx: &mut impl overseer::NetworkBridgeRxSenderTrait,
) {
	dispatch_validation_events_to_all(std::iter::once(event), ctx).await
}

async fn dispatch_collation_event_to_all(
	event: NetworkBridgeEvent<net_protocol::VersionedCollationProtocol>,
	ctx: &mut impl overseer::NetworkBridgeRxSenderTrait,
) {
	dispatch_collation_events_to_all(std::iter::once(event), ctx).await
}

fn dispatch_validation_event_to_all_unbounded(
	event: NetworkBridgeEvent<net_protocol::VersionedValidationProtocol>,
	sender: &mut impl overseer::NetworkBridgeRxSenderTrait,
) {
	event
		.focus()
		.ok()
		.map(StatementDistributionMessage::from)
		.and_then(|msg| Some(sender.send_unbounded_message(msg)));
	event
		.focus()
		.ok()
		.map(BitfieldDistributionMessage::from)
		.and_then(|msg| Some(sender.send_unbounded_message(msg)));
	event
		.focus()
		.ok()
		.map(ApprovalDistributionMessage::from)
		.and_then(|msg| Some(sender.send_unbounded_message(msg)));
	event
		.focus()
		.ok()
		.map(GossipSupportMessage::from)
		.and_then(|msg| Some(sender.send_unbounded_message(msg)));
}

fn dispatch_collation_event_to_all_unbounded(
	event: NetworkBridgeEvent<net_protocol::VersionedCollationProtocol>,
	sender: &mut impl overseer::NetworkBridgeRxSenderTrait,
) {
	if let Ok(msg) = event.focus() {
		sender.send_unbounded_message(CollatorProtocolMessage::NetworkBridgeUpdate(msg))
	}
}

async fn dispatch_validation_events_to_all<I>(
	events: I,
	sender: &mut impl overseer::NetworkBridgeRxSenderTrait,
) where
	I: IntoIterator<Item = NetworkBridgeEvent<net_protocol::VersionedValidationProtocol>>,
	I::IntoIter: Send,
{
	for event in events {
		sender
			.send_messages(event.focus().map(StatementDistributionMessage::from))
			.await;
		sender.send_messages(event.focus().map(BitfieldDistributionMessage::from)).await;
		sender.send_messages(event.focus().map(ApprovalDistributionMessage::from)).await;
		sender.send_messages(event.focus().map(GossipSupportMessage::from)).await;
	}
}

async fn dispatch_collation_events_to_all<I>(
	events: I,
	ctx: &mut impl overseer::NetworkBridgeRxSenderTrait,
) where
	I: IntoIterator<Item = NetworkBridgeEvent<net_protocol::VersionedCollationProtocol>>,
	I::IntoIter: Send,
{
	let messages_for = |event: NetworkBridgeEvent<net_protocol::VersionedCollationProtocol>| {
		event.focus().ok().map(|m| CollatorProtocolMessage::NetworkBridgeUpdate(m))
	};

	ctx.send_messages(events.into_iter().flat_map(messages_for)).await
}
