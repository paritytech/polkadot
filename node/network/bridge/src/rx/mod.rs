// Copyright (C) Parity Technologies (UK) Ltd.
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

use bytes::Bytes;
use parity_scale_codec::{Decode, DecodeAll};

use sc_network::service::traits::{
	MessageSink, NotificationEvent, NotificationService, ValidationResult,
};
use sp_consensus::SyncOracle;

use polkadot_node_network_protocol::{
	self as net_protocol,
	grid_topology::{SessionGridTopology, TopologyPeerInfo},
	peer_set::{CollationVersion, PeerSet, PeerSetProtocolNames, ValidationVersion},
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

use polkadot_primitives::{AuthorityDiscoveryId, BlockNumber, Hash, ValidatorIndex};

/// Peer set info for network initialization.
///
/// To be passed to [`FullNetworkConfiguration::add_notification_protocol`]().
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
	validation_service: Box<dyn NotificationService>,
	collation_service: Box<dyn NotificationService>,
	network_notification_sinks: Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
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
		mut notification_services: HashMap<PeerSet, Box<dyn NotificationService>>,
		network_notification_sinks: Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
	) -> Self {
		let shared = Shared::default();

		let validation_service = notification_services
			.remove(&PeerSet::Validation)
			.expect("validation protocol was enabled so `NotificationService` for it exists. qed");
		let collation_service = notification_services
			.remove(&PeerSet::Collation)
			.expect("collation protocol was enabled so `NotificationService` for it exists. qed");

		Self {
			network_service,
			authority_discovery_service,
			sync_oracle,
			shared,
			metrics,
			peerset_protocol_names,
			validation_service,
			collation_service,
			network_notification_sinks,
		}
	}
}

#[overseer::subsystem(NetworkBridgeRx, error = SubsystemError, prefix = self::overseer)]
impl<Net, AD, Context> NetworkBridgeRx<Net, AD>
where
	Net: Network + Sync,
	AD: validator_discovery::AuthorityDiscovery + Clone + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run_network`.
		let future = run_network_in(self, ctx)
			.map_err(|e| SubsystemError::with_origin("network-bridge", e))
			.boxed();
		SpawnedSubsystem { name: "network-bridge-rx-subsystem", future }
	}
}

/// Handle notification event received over the validation protocol.
async fn handle_validation_message<AD>(
	event: NotificationEvent,
	network_service: &mut impl Network,
	sender: &mut impl overseer::NetworkBridgeRxSenderTrait,
	authority_discovery_service: &mut AD,
	metrics: &Metrics,
	shared: &Shared,
	peerset_protocol_names: &PeerSetProtocolNames,
	notification_service: &mut Box<dyn NotificationService>,
	network_notification_sinks: &mut Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) -> Result<(), ()>
where
	AD: validator_discovery::AuthorityDiscovery + Send,
{
	match event {
		NotificationEvent::ValidateInboundSubstream { result_tx, .. } => {
			let _ = result_tx.send(ValidationResult::Accept);
		},
		NotificationEvent::NotificationStreamOpened {
			peer,
			handshake,
			negotiated_fallback,
			..
		} => {
			let role = match network_service.peer_role(peer, handshake) {
				Some(role) => ObservedRole::from(role),
				None => {
					gum::debug!(
						target: LOG_TARGET,
						?peer,
						"Failed to determine peer role",
					);
					return Ok(())
				},
			};

			let (peer_set, version) = {
				let (peer_set, version) = {
					let name = peerset_protocol_names.get_main_name(PeerSet::Validation);
					peerset_protocol_names
						.try_get_protocol(&name)
						.expect("validation protocol exists since it's enabled. qed")
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

							return Ok(())
						},
						Some((p2, v2)) => {
							if p2 != peer_set {
								gum::debug!(
									target: LOG_TARGET,
									fallback = &*fallback,
									fallback_peerset = ?p2,
									peerset = ?peer_set,
									"Fallback mismatched peer-set",
								);

								return Ok(())
							}

							(p2, v2)
						},
					}
				} else {
					(peer_set, version)
				}
			};

			// store the notification sink to `network_notification_sinks` so both `NetworkBridgeRx`
			// and `NetworkBridgeTx` can send messages to the peer.
			match notification_service.message_sink(&peer) {
				Some(sink) => {
					network_notification_sinks.lock().insert((peer_set, peer), sink);
				},
				None => {
					gum::warn!(
						target: LOG_TARGET,
						peer_set = ?peer_set,
						version = %version,
						peer = ?peer,
						role = ?role,
						"Message sink not available for peer",
					);
					return Ok(())
				},
			}

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
				let peer_map = &mut shared.validation_peers;

				match peer_map.entry(peer) {
					hash_map::Entry::Occupied(_) => return Ok(()),
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

			// TODO: store sink to networkservice?
			dispatch_validation_events_to_all(
				vec![
					NetworkBridgeEvent::PeerConnected(peer, role, version, maybe_authority),
					NetworkBridgeEvent::PeerViewChange(peer, View::default()),
				],
				sender,
			)
			.await;

			send_message(
				vec![peer],
				PeerSet::Validation,
				version,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(local_view),
				metrics,
				&network_notification_sinks,
			);
		},
		NotificationEvent::NotificationStreamClosed { peer } => {
			let (peer_set, version) = {
				let name = peerset_protocol_names.get_main_name(PeerSet::Validation);
				peerset_protocol_names
					.try_get_protocol(&name)
					.expect("validation protocol exists since it's enabled. qed")
			};

			gum::debug!(
				target: LOG_TARGET,
				action = "PeerDisconnected",
				peer_set = ?peer_set,
				peer = ?peer
			);

			let was_connected = {
				let mut shared = shared.0.lock();
				let peer_map = &mut shared.validation_peers;

				let w = peer_map.remove(&peer).is_some();

				metrics.on_peer_disconnected(peer_set, version);
				metrics.note_peer_count(peer_set, version, peer_map.len());

				w
			};

			network_notification_sinks.lock().remove(&(peer_set, peer));

			if was_connected && version == peer_set.get_main_version() {
				dispatch_validation_event_to_all(NetworkBridgeEvent::PeerDisconnected(peer), sender)
					.await
			}
		},
		NotificationEvent::NotificationReceived { peer, notification } => {
			if shared.0.lock().validation_peers.get(&peer).is_none() {
				// TODO: does this check make any sense?
				gum::debug!(target: LOG_TARGET, action = "ReportPeer");
				network_service.report_peer(peer, UNCONNECTED_PEERSET_COST.into());
				return Ok(())
			}

			gum::trace!(target: LOG_TARGET, action = "PeerMessage", ?peer);

			let (events, reports) = handle_v1_peer_messages::<protocol_v1::ValidationProtocol, _>(
				peer,
				PeerSet::Validation,
				&mut shared.0.lock().validation_peers,
				vec![Bytes::from(notification)],
				&metrics,
			);

			for report in reports {
				network_service.report_peer(peer, report.into());
			}

			dispatch_validation_events_to_all(events, sender).await;
		},
	}

	Ok(())
}

/// Handle notification event received over the collation protocol.
async fn handle_collation_message<AD>(
	event: NotificationEvent,
	network_service: &mut impl Network,
	sender: &mut impl overseer::NetworkBridgeRxSenderTrait,
	authority_discovery_service: &mut AD,
	metrics: &Metrics,
	shared: &Shared,
	peerset_protocol_names: &PeerSetProtocolNames,
	notification_service: &mut Box<dyn NotificationService>,
	network_notification_sinks: &mut Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) -> Result<(), ()>
where
	AD: validator_discovery::AuthorityDiscovery + Send,
{
	match event {
		NotificationEvent::ValidateInboundSubstream { result_tx, .. } => {
			let _ = result_tx.send(ValidationResult::Accept);
		},
		NotificationEvent::NotificationStreamOpened {
			peer,
			handshake,
			negotiated_fallback,
			..
		} => {
			let role = match network_service.peer_role(peer, handshake) {
				Some(role) => ObservedRole::from(role),
				None => {
					gum::debug!(
						target: LOG_TARGET,
						?peer,
						"Failed to determine peer role",
					);
					return Ok(())
				},
			};

			let (peer_set, version) = {
				let (peer_set, version) = {
					let name = peerset_protocol_names.get_main_name(PeerSet::Collation);
					peerset_protocol_names
						.try_get_protocol(&name)
						.expect("validation protocol exists since it's enabled. qed")
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

							return Ok(())
						},
						Some((p2, v2)) => {
							if p2 != peer_set {
								gum::debug!(
									target: LOG_TARGET,
									fallback = &*fallback,
									fallback_peerset = ?p2,
									peerset = ?peer_set,
									"Fallback mismatched peer-set",
								);

								return Ok(())
							}

							(p2, v2)
						},
					}
				} else {
					(peer_set, version)
				}
			};

			// store the notification sink to `network_notification_sinks` so both `NetworkBridgeRx`
			// and `NetworkBridgeTx` can send messages to the peer.
			match notification_service.message_sink(&peer) {
				Some(sink) => {
					network_notification_sinks.lock().insert((peer_set, peer), sink);
				},
				None => {
					gum::warn!(
						target: LOG_TARGET,
						peer_set = ?peer_set,
						version = %version,
						peer = ?peer,
						role = ?role,
						"Message sink not available for peer",
					);
					return Ok(())
				},
			}

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
				let peer_map = &mut shared.collation_peers;

				match peer_map.entry(peer) {
					hash_map::Entry::Occupied(_) => return Ok(()),
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

			dispatch_collation_events_to_all(
				vec![
					NetworkBridgeEvent::PeerConnected(peer, role, version, maybe_authority),
					NetworkBridgeEvent::PeerViewChange(peer, View::default()),
				],
				sender,
			)
			.await;

			send_message(
				vec![peer],
				PeerSet::Collation,
				version,
				WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(local_view),
				metrics,
				&network_notification_sinks,
			);
		},
		NotificationEvent::NotificationStreamClosed { peer } => {
			let (peer_set, version) = {
				let name = peerset_protocol_names.get_main_name(PeerSet::Collation);
				peerset_protocol_names
					.try_get_protocol(&name)
					.expect("validation protocol exists since it's enabled. qed")
			};

			gum::debug!(
				target: LOG_TARGET,
				action = "PeerDisconnected",
				peer_set = ?peer_set,
				peer = ?peer
			);

			let was_connected = {
				let mut shared = shared.0.lock();
				let peer_map = &mut shared.collation_peers;

				let w = peer_map.remove(&peer).is_some();

				metrics.on_peer_disconnected(peer_set, version);
				metrics.note_peer_count(peer_set, version, peer_map.len());

				w
			};

			network_notification_sinks.lock().remove(&(peer_set, peer));

			if was_connected && version == peer_set.get_main_version() {
				dispatch_collation_event_to_all(NetworkBridgeEvent::PeerDisconnected(peer), sender)
					.await
			}
		},
		NotificationEvent::NotificationReceived { peer, notification } => {
			// TODO: does this check make any sense?
			if shared.0.lock().collation_peers.get(&peer).is_none() {
				gum::debug!(target: LOG_TARGET, action = "ReportPeer");
				network_service.report_peer(peer, UNCONNECTED_PEERSET_COST.into());
				return Ok(())
			}

			gum::trace!(target: LOG_TARGET, action = "PeerMessages", ?peer);

			let (events, reports) = handle_v1_peer_messages::<protocol_v1::CollationProtocol, _>(
				peer,
				PeerSet::Collation,
				&mut shared.0.lock().collation_peers,
				vec![Bytes::from(notification)],
				&metrics,
			);

			for report in reports {
				network_service.report_peer(peer, report.into());
			}

			dispatch_collation_events_to_all(events, sender).await;
		},
	}

	Ok(())
}

async fn handle_network_messages<AD>(
	mut sender: impl overseer::NetworkBridgeRxSenderTrait,
	mut network_service: impl Network,
	mut validation_service: Box<dyn NotificationService>,
	mut collation_service: Box<dyn NotificationService>,
	mut authority_discovery_service: AD,
	metrics: Metrics,
	shared: Shared,
	peerset_protocol_names: PeerSetProtocolNames,
	mut network_notification_sinks: Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) -> Result<(), Error>
where
	AD: validator_discovery::AuthorityDiscovery + Send,
{
	loop {
		futures::select! {
			event = validation_service.next_event().fuse() => match event {
				Some(event) => if let Err(_error) = handle_validation_message(
					event,
					&mut network_service,
					&mut sender,
					&mut authority_discovery_service,
					&metrics,
					&shared,
					&peerset_protocol_names,
					&mut validation_service,
					&mut network_notification_sinks,
				).await {
					// TODO: log error
				}
				None => return Err(Error::EventStreamConcluded),
			},
			event = collation_service.next_event().fuse() => match event {
				Some(event) => if let Err(_error) = handle_collation_message(
					event,
					&mut network_service,
					&mut sender,
					&mut authority_discovery_service,
					&metrics,
					&shared,
					&peerset_protocol_names,
					&mut collation_service,
					&mut network_notification_sinks,
				).await {
					// TODO: log error
				}
				None => return Err(Error::EventStreamConcluded),
			}
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
async fn run_incoming_orchestra_signals<Context, AD>(
	mut ctx: Context,
	mut authority_discovery_service: AD,
	shared: Shared,
	sync_oracle: Box<dyn SyncOracle + Send>,
	metrics: Metrics,
	network_notification_sinks: Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) -> Result<(), Error>
where
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
			FromOrchestra::Communication {
				msg: NetworkBridgeRxMessage::UpdatedAuthorityIds { peer_id, authority_ids },
			} => {
				gum::debug!(
					target: LOG_TARGET,
					action = "UpdatedAuthorityIds",
					?peer_id,
					?authority_ids,
					"`AuthorityDiscoveryId`s have changed",
				);
				// using unbounded send to avoid cycles
				// the messages are sent only once per session up to one per peer
				dispatch_collation_event_to_all_unbounded(
					NetworkBridgeEvent::UpdatedAuthorityIds(peer_id, authority_ids.clone()),
					ctx.sender(),
				);
				dispatch_validation_event_to_all_unbounded(
					NetworkBridgeEvent::UpdatedAuthorityIds(peer_id, authority_ids),
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
							&mut ctx,
							&live_heads,
							&shared,
							finalized_number,
							&metrics,
							&network_notification_sinks,
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
#[overseer::contextbounds(NetworkBridgeRx, prefix = self::overseer)]
async fn run_network_in<N, AD, Context>(
	bridge: NetworkBridgeRx<N, AD>,
	mut ctx: Context,
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
		validation_service,
		collation_service,
		network_notification_sinks,
	} = bridge;

	let (task, network_event_handler) = handle_network_messages(
		ctx.sender().clone(),
		network_service.clone(),
		validation_service,
		collation_service,
		authority_discovery_service.clone(),
		metrics.clone(),
		shared.clone(),
		peerset_protocol_names.clone(),
		network_notification_sinks.clone(),
	)
	.remote_handle();

	ctx.spawn_blocking("network-bridge-in-network-worker", Box::pin(task))?;
	futures::pin_mut!(network_event_handler);

	let orchestra_signal_handler = run_incoming_orchestra_signals(
		ctx,
		authority_discovery_service,
		shared,
		sync_oracle,
		metrics,
		network_notification_sinks,
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
fn update_our_view<Context>(
	ctx: &mut Context,
	live_heads: &[ActivatedLeaf],
	shared: &Shared,
	finalized_number: BlockNumber,
	metrics: &Metrics,
	network_notification_sinks: &Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) {
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
		validation_peers,
		WireMessage::ViewUpdate(new_view.clone()),
		metrics,
		network_notification_sinks,
	);

	send_collation_message_v1(
		collation_peers,
		WireMessage::ViewUpdate(new_view),
		metrics,
		network_notification_sinks,
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
	peers: Vec<PeerId>,
	message: WireMessage<protocol_v1::ValidationProtocol>,
	metrics: &Metrics,
	network_notification_sinks: &Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) {
	send_message(
		peers,
		PeerSet::Validation,
		ValidationVersion::V1.into(),
		message,
		metrics,
		network_notification_sinks,
	);
}

fn send_collation_message_v1(
	peers: Vec<PeerId>,
	message: WireMessage<protocol_v1::CollationProtocol>,
	metrics: &Metrics,
	network_notification_sinks: &Arc<Mutex<HashMap<(PeerSet, PeerId), Box<dyn MessageSink>>>>,
) {
	send_message(
		peers,
		PeerSet::Collation,
		CollationVersion::V1.into(),
		message,
		metrics,
		&network_notification_sinks,
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
