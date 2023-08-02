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

//! The bitfield distribution
//!
//! In case this node is a validator, gossips its own signed availability bitfield
//! for a particular relay parent.
//! Independently of that, gossips on received messages from peers to other interested peers.

#![deny(unused_crate_dependencies)]

use always_assert::never;
use futures::{channel::oneshot, FutureExt};

use polkadot_node_network_protocol::{
	self as net_protocol,
	grid_topology::{
		GridNeighbors, RandomRouting, RequiredRouting, SessionBoundGridTopologyStorage,
	},
	peer_set::{ProtocolVersion, ValidationVersion},
	v1 as protocol_v1, vstaging as protocol_vstaging, OurView, PeerId,
	UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_subsystem::{
	jaeger, messages::*, overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, PerLeafSpan,
	SpawnedSubsystem, SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	self as util,
	reputation::{ReputationAggregator, REPUTATION_CHANGE_INTERVAL},
};

use futures::select;
use polkadot_primitives::{Hash, SignedAvailabilityBitfield, SigningContext, ValidatorId};
use rand::{CryptoRng, Rng, SeedableRng};
use std::{
	collections::{HashMap, HashSet},
	time::Duration,
};

use self::metrics::Metrics;

mod metrics;

#[cfg(test)]
mod tests;

const COST_SIGNATURE_INVALID: Rep = Rep::CostMajor("Bitfield signature invalid");
const COST_VALIDATOR_INDEX_INVALID: Rep = Rep::CostMajor("Bitfield validator index invalid");
const COST_MISSING_PEER_SESSION_KEY: Rep = Rep::CostMinor("Missing peer session key");
const COST_NOT_IN_VIEW: Rep = Rep::CostMinor("Not interested in that parent hash");
const COST_PEER_DUPLICATE_MESSAGE: Rep =
	Rep::CostMinorRepeated("Peer sent the same message multiple times");
const BENEFIT_VALID_MESSAGE_FIRST: Rep =
	Rep::BenefitMinorFirst("Valid message with new information");
const BENEFIT_VALID_MESSAGE: Rep = Rep::BenefitMinor("Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BitfieldGossipMessage {
	/// The relay parent this message is relative to.
	relay_parent: Hash,
	/// The actual signed availability bitfield.
	signed_availability: SignedAvailabilityBitfield,
}

impl BitfieldGossipMessage {
	fn into_validation_protocol(
		self,
		recipient_version: ProtocolVersion,
	) -> net_protocol::VersionedValidationProtocol {
		self.into_network_message(recipient_version).into()
	}

	fn into_network_message(
		self,
		recipient_version: ProtocolVersion,
	) -> net_protocol::BitfieldDistributionMessage {
		match ValidationVersion::try_from(recipient_version).ok() {
			Some(ValidationVersion::V1) =>
				Versioned::V1(protocol_v1::BitfieldDistributionMessage::Bitfield(
					self.relay_parent,
					self.signed_availability.into(),
				)),
			Some(ValidationVersion::VStaging) =>
				Versioned::VStaging(protocol_vstaging::BitfieldDistributionMessage::Bitfield(
					self.relay_parent,
					self.signed_availability.into(),
				)),
			None => {
				never!("Peers should only have supported protocol versions.");

				gum::warn!(
					target: LOG_TARGET,
					version = ?recipient_version,
					"Unknown protocol version provided for message recipient"
				);

				// fall back to v1 to avoid
				Versioned::V1(protocol_v1::BitfieldDistributionMessage::Bitfield(
					self.relay_parent,
					self.signed_availability.into(),
				))
			},
		}
	}
}

/// Data stored on a per-peer basis.
#[derive(Debug)]
pub struct PeerData {
	/// The peer's view.
	view: View,
	/// The peer's protocol version.
	version: ProtocolVersion,
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Debug)]
struct ProtocolState {
	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_data: HashMap<PeerId, PeerData>,

	/// The current and previous gossip topologies
	topologies: SessionBoundGridTopologyStorage,

	/// Our current view.
	view: OurView,

	/// Additional data particular to a relay parent.
	per_relay_parent: HashMap<Hash, PerRelayParentData>,

	/// Aggregated reputation change
	reputation: ReputationAggregator,
}

/// Data for a particular relay parent.
#[derive(Debug)]
struct PerRelayParentData {
	/// Signing context for a particular relay parent.
	signing_context: SigningContext,

	/// Set of validators for a particular relay parent.
	validator_set: Vec<ValidatorId>,

	/// Set of validators for a particular relay parent for which we
	/// received a valid `BitfieldGossipMessage`.
	/// Also serves as the list of known messages for peers connecting
	/// after bitfield gossips were already received.
	one_per_validator: HashMap<ValidatorId, BitfieldGossipMessage>,

	/// Avoid duplicate message transmission to our peers.
	message_sent_to_peer: HashMap<PeerId, HashSet<ValidatorId>>,

	/// Track messages that were already received by a peer
	/// to prevent flooding.
	message_received_from_peer: HashMap<PeerId, HashSet<ValidatorId>>,

	/// The span for this leaf/relay parent.
	span: PerLeafSpan,
}

impl PerRelayParentData {
	/// Create a new instance.
	fn new(
		signing_context: SigningContext,
		validator_set: Vec<ValidatorId>,
		span: PerLeafSpan,
	) -> Self {
		Self {
			signing_context,
			validator_set,
			span,
			one_per_validator: Default::default(),
			message_sent_to_peer: Default::default(),
			message_received_from_peer: Default::default(),
		}
	}

	/// Determines if that particular message signed by a
	/// validator is needed by the given peer.
	fn message_from_validator_needed_by_peer(
		&self,
		peer: &PeerId,
		signed_by: &ValidatorId,
	) -> bool {
		self.message_sent_to_peer
			.get(peer)
			.map(|pubkeys| !pubkeys.contains(signed_by))
			.unwrap_or(true) &&
			self.message_received_from_peer
				.get(peer)
				.map(|pubkeys| !pubkeys.contains(signed_by))
				.unwrap_or(true)
	}
}

const LOG_TARGET: &str = "parachain::bitfield-distribution";

/// The bitfield distribution subsystem.
pub struct BitfieldDistribution {
	metrics: Metrics,
}

#[overseer::contextbounds(BitfieldDistribution, prefix = self::overseer)]
impl BitfieldDistribution {
	/// Create a new instance of the `BitfieldDistribution` subsystem.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, ctx: Context) {
		let mut state = ProtocolState::default();
		let mut rng = rand::rngs::StdRng::from_entropy();
		self.run_inner(ctx, &mut state, REPUTATION_CHANGE_INTERVAL, &mut rng).await
	}

	async fn run_inner<Context>(
		self,
		mut ctx: Context,
		state: &mut ProtocolState,
		reputation_interval: Duration,
		rng: &mut (impl CryptoRng + Rng),
	) {
		// work: process incoming messages from the overseer and process accordingly.

		let new_reputation_delay = || futures_timer::Delay::new(reputation_interval).fuse();
		let mut reputation_delay = new_reputation_delay();

		loop {
			select! {
				_ = reputation_delay => {
					state.reputation.send(ctx.sender()).await;
					reputation_delay = new_reputation_delay();
				},
				message = ctx.recv().fuse() => {
					let message = match message {
						Ok(message) => message,
						Err(err) => {
							gum::error!(
								target: LOG_TARGET,
								?err,
								"Failed to receive a message from Overseer, exiting"
							);
							return
						},
					};
					match message {
						FromOrchestra::Communication {
							msg:
								BitfieldDistributionMessage::DistributeBitfield(
									relay_parent,
									signed_availability,
								),
						} => {
							gum::trace!(target: LOG_TARGET, ?relay_parent, "Processing DistributeBitfield");
							handle_bitfield_distribution(
								&mut ctx,
								state,
								&self.metrics,
								relay_parent,
								signed_availability,
								rng,
							)
							.await;
						},
						FromOrchestra::Communication {
							msg: BitfieldDistributionMessage::NetworkBridgeUpdate(event),
						} => {
							gum::trace!(target: LOG_TARGET, "Processing NetworkMessage");
							// a network message was received
							handle_network_msg(&mut ctx, state, &self.metrics, event, rng).await;
						},
						FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
							activated,
							..
						})) => {
							let _timer = self.metrics.time_active_leaves_update();

							if let Some(activated) = activated {
								let relay_parent = activated.hash;

								gum::trace!(target: LOG_TARGET, ?relay_parent, "activated");
								let span = PerLeafSpan::new(activated.span, "bitfield-distribution");
								let _span = span.child("query-basics");

								// query validator set and signing context per relay_parent once only
								match query_basics(&mut ctx, relay_parent).await {
									Ok(Some((validator_set, signing_context))) => {
										// If our runtime API fails, we don't take down the node,
										// but we might alter peers' reputations erroneously as a result
										// of not having the correct bookkeeping. If we have lost a race
										// with state pruning, it is unlikely that peers will be sending
										// us anything to do with this relay-parent anyway.
										let _ = state.per_relay_parent.insert(
											relay_parent,
											PerRelayParentData::new(signing_context, validator_set, span),
										);
									},
									Err(err) => {
										gum::warn!(target: LOG_TARGET, ?err, "query_basics has failed");
									},
									_ => {},
								}
							}
						},
						FromOrchestra::Signal(OverseerSignal::BlockFinalized(hash, number)) => {
							gum::trace!(target: LOG_TARGET, ?hash, %number, "block finalized");
						},
						FromOrchestra::Signal(OverseerSignal::Conclude) => {
							gum::info!(target: LOG_TARGET, "Conclude");
							return
						},
					}
				}
			}
		}
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
	reputation: &mut ReputationAggregator,
	sender: &mut impl overseer::BitfieldDistributionSenderTrait,
	relay_parent: Hash,
	peer: PeerId,
	rep: Rep,
) {
	gum::trace!(target: LOG_TARGET, ?relay_parent, ?rep, %peer, "reputation change");

	reputation.modify(sender, peer, rep).await;
}
/// Distribute a given valid and signature checked bitfield message.
///
/// For this variant the source is this node.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn handle_bitfield_distribution<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	relay_parent: Hash,
	signed_availability: SignedAvailabilityBitfield,
	rng: &mut (impl CryptoRng + Rng),
) {
	let _timer = metrics.time_handle_bitfield_distribution();

	// Ignore anything the overseer did not tell this subsystem to work on
	let mut job_data = state.per_relay_parent.get_mut(&relay_parent);
	let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
		job_data
	} else {
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			"Not supposed to work on relay parent related data",
		);

		return
	};

	let session_idx = job_data.signing_context.session_index;
	let validator_set = &job_data.validator_set;
	if validator_set.is_empty() {
		gum::debug!(target: LOG_TARGET, ?relay_parent, "validator set is empty");
		return
	}

	let validator_index = signed_availability.validator_index();
	let validator = if let Some(validator) = validator_set.get(validator_index.0 as usize) {
		validator.clone()
	} else {
		gum::debug!(target: LOG_TARGET, validator_index = ?validator_index.0, "Could not find a validator for index");
		return
	};

	let msg = BitfieldGossipMessage { relay_parent, signed_availability };
	let topology = state.topologies.get_topology_or_fallback(session_idx).local_grid_neighbors();
	let required_routing = topology.required_routing_by_index(validator_index, true);

	relay_message(
		ctx,
		job_data,
		topology,
		&mut state.peer_data,
		validator,
		msg,
		required_routing,
		rng,
	)
	.await;

	metrics.on_own_bitfield_sent();
}

/// Distribute a given valid and signature checked bitfield message.
///
/// Can be originated by another subsystem or received via network from another peer.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn relay_message<Context>(
	ctx: &mut Context,
	job_data: &mut PerRelayParentData,
	topology_neighbors: &GridNeighbors,
	peers: &mut HashMap<PeerId, PeerData>,
	validator: ValidatorId,
	message: BitfieldGossipMessage,
	required_routing: RequiredRouting,
	rng: &mut (impl CryptoRng + Rng),
) {
	let relay_parent = message.relay_parent;
	let span = job_data.span.child("relay-msg");

	let _span = span.child("provisionable");
	// notify the overseer about a new and valid signed bitfield
	ctx.send_message(ProvisionerMessage::ProvisionableData(
		relay_parent,
		ProvisionableData::Bitfield(relay_parent, message.signed_availability.clone()),
	))
	.await;

	drop(_span);
	let total_peers = peers.len();
	let mut random_routing: RandomRouting = Default::default();

	let _span = span.child("interested-peers");
	// pass on the bitfield distribution to all interested peers
	let interested_peers = peers
		.iter()
		.filter_map(|(peer, data)| {
			// check interest in the peer in this message's relay parent
			if data.view.contains(&message.relay_parent) {
				let message_needed =
					job_data.message_from_validator_needed_by_peer(&peer, &validator);
				if message_needed {
					let in_topology = topology_neighbors.route_to_peer(required_routing, &peer);
					let need_routing = in_topology || {
						let route_random = random_routing.sample(total_peers, rng);
						if route_random {
							random_routing.inc_sent();
						}

						route_random
					};

					if need_routing {
						Some((*peer, data.version))
					} else {
						None
					}
				} else {
					None
				}
			} else {
				None
			}
		})
		.collect::<Vec<(PeerId, ProtocolVersion)>>();

	interested_peers.iter().for_each(|(peer, _)| {
		// track the message as sent for this peer
		job_data
			.message_sent_to_peer
			.entry(*peer)
			.or_default()
			.insert(validator.clone());
	});

	drop(_span);

	if interested_peers.is_empty() {
		gum::trace!(
			target: LOG_TARGET,
			?relay_parent,
			"no peers are interested in gossip for relay parent",
		);
	} else {
		let _span = span.child("gossip");

		let filter_by_version = |peers: &[(PeerId, ProtocolVersion)],
		                         version: ValidationVersion| {
			peers
				.iter()
				.filter(|(_, v)| v == &version.into())
				.map(|(peer_id, _)| *peer_id)
				.collect::<Vec<_>>()
		};

		let v1_interested_peers = filter_by_version(&interested_peers, ValidationVersion::V1);
		let vstaging_interested_peers =
			filter_by_version(&interested_peers, ValidationVersion::VStaging);

		if !v1_interested_peers.is_empty() {
			ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				v1_interested_peers,
				message.clone().into_validation_protocol(ValidationVersion::V1.into()),
			))
			.await;
		}

		if !vstaging_interested_peers.is_empty() {
			ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				vstaging_interested_peers,
				message.into_validation_protocol(ValidationVersion::VStaging.into()),
			))
			.await
		}
	}
}

/// Handle an incoming message from a peer.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	origin: PeerId,
	message: net_protocol::BitfieldDistributionMessage,
	rng: &mut (impl CryptoRng + Rng),
) {
	let (relay_parent, bitfield) = match message {
		Versioned::V1(protocol_v1::BitfieldDistributionMessage::Bitfield(
			relay_parent,
			bitfield,
		)) => (relay_parent, bitfield),
		Versioned::VStaging(protocol_vstaging::BitfieldDistributionMessage::Bitfield(
			relay_parent,
			bitfield,
		)) => (relay_parent, bitfield),
	};

	gum::trace!(
		target: LOG_TARGET,
		peer = %origin,
		?relay_parent,
		"received bitfield gossip from peer"
	);
	// we don't care about this, not part of our view.
	if !state.view.contains(&relay_parent) {
		modify_reputation(
			&mut state.reputation,
			ctx.sender(),
			relay_parent,
			origin,
			COST_NOT_IN_VIEW,
		)
		.await;
		return
	}

	// Ignore anything the overseer did not tell this subsystem to work on.
	let mut job_data = state.per_relay_parent.get_mut(&relay_parent);
	let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
		job_data
	} else {
		modify_reputation(
			&mut state.reputation,
			ctx.sender(),
			relay_parent,
			origin,
			COST_NOT_IN_VIEW,
		)
		.await;
		return
	};

	let validator_index = bitfield.unchecked_validator_index();

	let mut _span = job_data
		.span
		.child("msg-received")
		.with_peer_id(&origin)
		.with_relay_parent(relay_parent)
		.with_claimed_validator_index(validator_index)
		.with_stage(jaeger::Stage::BitfieldDistribution);

	let validator_set = &job_data.validator_set;
	if validator_set.is_empty() {
		gum::trace!(target: LOG_TARGET, ?relay_parent, ?origin, "Validator set is empty",);
		modify_reputation(
			&mut state.reputation,
			ctx.sender(),
			relay_parent,
			origin,
			COST_MISSING_PEER_SESSION_KEY,
		)
		.await;
		return
	}

	// Use the (untrusted) validator index provided by the signed payload
	// and see if that one actually signed the availability bitset.
	let signing_context = job_data.signing_context.clone();
	let validator = if let Some(validator) = validator_set.get(validator_index.0 as usize) {
		validator.clone()
	} else {
		modify_reputation(
			&mut state.reputation,
			ctx.sender(),
			relay_parent,
			origin,
			COST_VALIDATOR_INDEX_INVALID,
		)
		.await;
		return
	};

	// Check if the peer already sent us a message for the validator denoted in the message earlier.
	// Must be done after validator index verification, in order to avoid storing an unbounded
	// number of set entries.
	let received_set = job_data.message_received_from_peer.entry(origin).or_default();

	if !received_set.contains(&validator) {
		received_set.insert(validator.clone());
	} else {
		gum::trace!(target: LOG_TARGET, ?validator_index, ?origin, "Duplicate message");
		modify_reputation(
			&mut state.reputation,
			ctx.sender(),
			relay_parent,
			origin,
			COST_PEER_DUPLICATE_MESSAGE,
		)
		.await;
		return
	};

	let one_per_validator = &mut (job_data.one_per_validator);

	// relay a message received from a validator at most _once_
	if let Some(old_message) = one_per_validator.get(&validator) {
		gum::trace!(
			target: LOG_TARGET,
			?validator_index,
			"already received a message for validator",
		);
		if old_message.signed_availability.as_unchecked() == &bitfield {
			modify_reputation(
				&mut state.reputation,
				ctx.sender(),
				relay_parent,
				origin,
				BENEFIT_VALID_MESSAGE,
			)
			.await;
		}
		return
	}
	let signed_availability = match bitfield.try_into_checked(&signing_context, &validator) {
		Err(_) => {
			modify_reputation(
				&mut state.reputation,
				ctx.sender(),
				relay_parent,
				origin,
				COST_SIGNATURE_INVALID,
			)
			.await;
			return
		},
		Ok(bitfield) => bitfield,
	};

	let message = BitfieldGossipMessage { relay_parent, signed_availability };

	let topology = state
		.topologies
		.get_topology_or_fallback(job_data.signing_context.session_index)
		.local_grid_neighbors();
	let required_routing = topology.required_routing_by_index(validator_index, false);

	metrics.on_bitfield_received();
	one_per_validator.insert(validator.clone(), message.clone());

	relay_message(
		ctx,
		job_data,
		topology,
		&mut state.peer_data,
		validator,
		message,
		required_routing,
		rng,
	)
	.await;

	modify_reputation(
		&mut state.reputation,
		ctx.sender(),
		relay_parent,
		origin,
		BENEFIT_VALID_MESSAGE_FIRST,
	)
	.await
}

/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	bridge_message: NetworkBridgeEvent<net_protocol::BitfieldDistributionMessage>,
	rng: &mut (impl CryptoRng + Rng),
) {
	let _timer = metrics.time_handle_network_msg();

	match bridge_message {
		NetworkBridgeEvent::PeerConnected(peer, role, version, _) => {
			gum::trace!(target: LOG_TARGET, ?peer, ?role, "Peer connected");
			// insert if none already present
			state
				.peer_data
				.entry(peer)
				.or_insert_with(|| PeerData { view: View::default(), version });
		},
		NetworkBridgeEvent::PeerDisconnected(peer) => {
			gum::trace!(target: LOG_TARGET, ?peer, "Peer disconnected");
			// get rid of superfluous data
			state.peer_data.remove(&peer);
		},
		NetworkBridgeEvent::NewGossipTopology(gossip_topology) => {
			let session_index = gossip_topology.session;
			let new_topology = gossip_topology.topology;
			let prev_neighbors =
				state.topologies.get_current_topology().local_grid_neighbors().clone();

			state.topologies.update_topology(
				session_index,
				new_topology,
				gossip_topology.local_index,
			);
			let current_topology = state.topologies.get_current_topology();

			let newly_added = current_topology.local_grid_neighbors().peers_diff(&prev_neighbors);

			gum::debug!(
				target: LOG_TARGET,
				?session_index,
				newly_added_peers = ?newly_added.len(),
				"New gossip topology received",
			);

			for new_peer in newly_added {
				let old_view = match state.peer_data.get_mut(&new_peer) {
					Some(d) => {
						// in case we already knew that peer in the past
						// it might have had an existing view, we use to initialize
						// and minimize the delta on `PeerViewChange` to be sent
						std::mem::replace(&mut d.view, Default::default())
					},
					None => {
						// For peers which are currently unknown, we'll send topology-related
						// messages to them when they connect and send their first view update.
						continue
					},
				};

				handle_peer_view_change(ctx, state, new_peer, old_view, rng).await;
			}
		},
		NetworkBridgeEvent::PeerViewChange(peerid, new_view) => {
			gum::trace!(target: LOG_TARGET, ?peerid, ?new_view, "Peer view change");
			handle_peer_view_change(ctx, state, peerid, new_view, rng).await;
		},
		NetworkBridgeEvent::OurViewChange(new_view) => {
			gum::trace!(target: LOG_TARGET, ?new_view, "Our view change");
			handle_our_view_change(state, new_view);
		},
		NetworkBridgeEvent::PeerMessage(remote, message) =>
			process_incoming_peer_message(ctx, state, metrics, remote, message, rng).await,
		NetworkBridgeEvent::UpdatedAuthorityIds { .. } => {
			// The bitfield-distribution subsystem doesn't deal with `AuthorityDiscoveryId`s.
		},
	}
}

/// Handle the changes necessary when our view changes.
fn handle_our_view_change(state: &mut ProtocolState, view: OurView) {
	let old_view = std::mem::replace(&mut (state.view), view);

	for added in state.view.difference(&old_view) {
		if !state.per_relay_parent.contains_key(&added) {
			// Is guaranteed to be handled in `ActiveHead` update
			// so this should never happen.
			gum::error!(
				target: LOG_TARGET,
				%added,
				"Our view contains {}, but not in active heads",
				&added
			);
		}
	}
	for removed in old_view.difference(&state.view) {
		// cleanup relay parents we are not interested in any more
		let _ = state.per_relay_parent.remove(&removed);
	}
}

// Send the difference between two views which were not sent
// to that particular peer.
//
// This requires that there is an entry in the `peer_data` field for the
// peer.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn handle_peer_view_change<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	view: View,
	rng: &mut (impl CryptoRng + Rng),
) {
	let peer_data = match state.peer_data.get_mut(&origin) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				peer = ?origin,
				"Attempted to update peer view for unknown peer."
			);

			return
		},
		Some(pd) => pd,
	};

	let added = peer_data.view.replace_difference(view).cloned().collect::<Vec<_>>();

	let topology = state.topologies.get_current_topology().local_grid_neighbors();
	let is_gossip_peer = topology.route_to_peer(RequiredRouting::GridXY, &origin);
	let lucky = is_gossip_peer ||
		util::gen_ratio_rng(
			util::MIN_GOSSIP_PEERS.saturating_sub(topology.len()),
			util::MIN_GOSSIP_PEERS,
			rng,
		);

	if !lucky {
		gum::trace!(target: LOG_TARGET, ?origin, "Peer view change is ignored");
		return
	}

	// Send all messages we've seen before and the peer is now interested
	// in to that peer.
	let delta_set: Vec<(ValidatorId, BitfieldGossipMessage)> = added
		.into_iter()
		.filter_map(|new_relay_parent_interest| {
			if let Some(job_data) = state.per_relay_parent.get(&new_relay_parent_interest) {
				// Send all jointly known messages for a validator (given the current relay parent)
				// to the peer `origin`...
				let one_per_validator = job_data.one_per_validator.clone();
				Some(one_per_validator.into_iter().filter(move |(validator, _message)| {
					// ..except for the ones the peer already has.
					job_data.message_from_validator_needed_by_peer(&origin, validator)
				}))
			} else {
				// A relay parent is in the peers view, which is not in ours, ignore those.
				None
			}
		})
		.flatten()
		.collect();

	for (validator, message) in delta_set.into_iter() {
		send_tracked_gossip_message(ctx, state, origin, validator, message).await;
	}
}

/// Send a gossip message and track it in the per relay parent data.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn send_tracked_gossip_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	dest: PeerId,
	validator: ValidatorId,
	message: BitfieldGossipMessage,
) {
	let job_data = if let Some(job_data) = state.per_relay_parent.get_mut(&message.relay_parent) {
		job_data
	} else {
		return
	};

	let _span = job_data.span.child("gossip");
	gum::trace!(
		target: LOG_TARGET,
		?dest,
		?validator,
		relay_parent = ?message.relay_parent,
		"Sending gossip message"
	);

	let version =
		if let Some(peer_data) = state.peer_data.get(&dest) { peer_data.version } else { return };

	job_data.message_sent_to_peer.entry(dest).or_default().insert(validator.clone());

	ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
		vec![dest],
		message.into_validation_protocol(version),
	))
	.await;
}

#[overseer::subsystem(BitfieldDistribution, error=SubsystemError, prefix=self::overseer)]
impl<Context> BitfieldDistribution {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self.run(ctx).map(|_| Ok(())).boxed();

		SpawnedSubsystem { name: "bitfield-distribution-subsystem", future }
	}
}

/// Query our validator set and signing context for a particular relay parent.
#[overseer::contextbounds(BitfieldDistribution, prefix=self::overseer)]
async fn query_basics<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> SubsystemResult<Option<(Vec<ValidatorId>, SigningContext)>> {
	let (validators_tx, validators_rx) = oneshot::channel();
	let (session_tx, session_rx) = oneshot::channel();

	// query validators
	ctx.send_message(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::Validators(validators_tx),
	))
	.await;

	// query signing context
	ctx.send_message(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::SessionIndexForChild(session_tx),
	))
	.await;

	match (validators_rx.await?, session_rx.await?) {
		(Ok(validators), Ok(session_index)) =>
			Ok(Some((validators, SigningContext { parent_hash: relay_parent, session_index }))),
		(Err(err), _) | (_, Err(err)) => {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				?err,
				"Failed to fetch basics from runtime API"
			);
			Ok(None)
		},
	}
}
