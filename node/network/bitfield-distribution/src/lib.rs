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

//! The bitfield distribution
//!
//! In case this node is a validator, gossips its own signed availability bitfield
//! for a particular relay parent.
//! Independently of that, gossips on received messages from peers to other interested peers.

use codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt};

use log::{trace, warn};
use polkadot_subsystem::messages::*;
use polkadot_subsystem::{
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{Hash, SignedAvailabilityBitfield, SigningContext, ValidatorId};
use polkadot_node_network_protocol::{v1 as protocol_v1, PeerId, NetworkBridgeEvent, View, ReputationChange};
use std::collections::{HashMap, HashSet};

const COST_SIGNATURE_INVALID: ReputationChange =
	ReputationChange::new(-100, "Bitfield signature invalid");
const COST_VALIDATOR_INDEX_INVALID: ReputationChange =
	ReputationChange::new(-100, "Bitfield validator index invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
	ReputationChange::new(-133, "Missing peer session key");
const COST_NOT_IN_VIEW: ReputationChange =
	ReputationChange::new(-51, "Not interested in that parent hash");
const COST_PEER_DUPLICATE_MESSAGE: ReputationChange =
	ReputationChange::new(-500, "Peer sent the same message multiple times");
const BENEFIT_VALID_MESSAGE_FIRST: ReputationChange =
	ReputationChange::new(15, "Valid message with new information");
const BENEFIT_VALID_MESSAGE: ReputationChange =
	ReputationChange::new(10, "Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
struct BitfieldGossipMessage {
	/// The relay parent this message is relative to.
	relay_parent: Hash,
	/// The actual signed availability bitfield.
	signed_availability: SignedAvailabilityBitfield,
}

impl BitfieldGossipMessage {
	fn into_validation_protocol(self) -> protocol_v1::ValidationProtocol {
		protocol_v1::ValidationProtocol::BitfieldDistribution(
			self.into_network_message()
		)
	}

	fn into_network_message(self)
		-> protocol_v1::BitfieldDistributionMessage
	{
		protocol_v1::BitfieldDistributionMessage::Bitfield(
			self.relay_parent,
			self.signed_availability,
		)
	}
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
	/// track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our current view.
	view: View,

	/// Additional data particular to a relay parent.
	per_relay_parent: HashMap<Hash, PerRelayParentData>,
}

/// Data for a particular relay parent.
#[derive(Debug, Clone, Default)]
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
}

impl PerRelayParentData {
	/// Determines if that particular message signed by a validator is needed by the given peer.
	fn message_from_validator_needed_by_peer(
		&self,
		peer: &PeerId,
		validator: &ValidatorId,
	) -> bool {
		if let Some(set) = self.message_sent_to_peer.get(peer) {
			!set.contains(validator)
		} else {
			false
		}
	}
}

const TARGET: &'static str = "bitd";

/// The bitfield distribution subsystem.
pub struct BitfieldDistribution {
	metrics: Metrics,
}

impl BitfieldDistribution {
	/// Create a new instance of the `BitfieldDistribution` subsystem.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, mut ctx: Context) -> SubsystemResult<()>
	where
		Context: SubsystemContext<Message = BitfieldDistributionMessage>,
	{
		// work: process incoming messages from the overseer and process accordingly.
		let mut state = ProtocolState::default();
		loop {
			let message = ctx.recv().await?;
			match message {
				FromOverseer::Communication {
					msg: BitfieldDistributionMessage::DistributeBitfield(hash, signed_availability),
				} => {
					trace!(target: TARGET, "Processing DistributeBitfield");
					handle_bitfield_distribution(&mut ctx, &mut state, &self.metrics, hash, signed_availability)
						.await?;
				}
				FromOverseer::Communication {
					msg: BitfieldDistributionMessage::NetworkBridgeUpdateV1(event),
				} => {
					trace!(target: TARGET, "Processing NetworkMessage");
					// a network message was received
					if let Err(e) = handle_network_msg(&mut ctx, &mut state, &self.metrics, event).await {
						warn!(target: TARGET, "Failed to handle incomming network messages: {:?}", e);
					}
				}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated })) => {
					for relay_parent in activated {
						trace!(target: TARGET, "Start {:?}", relay_parent);
						// query basic system parameters once
						if let Some((validator_set, signing_context)) =
							query_basics(&mut ctx, relay_parent).await?
						{
							// If our runtime API fails, we don't take down the node,
							// but we might alter peers' reputations erroneously as a result
							// of not having the correct bookkeeping. If we have lost a race
							// with state pruning, it is unlikely that peers will be sending
							// us anything to do with this relay-parent anyway.
							let _ = state.per_relay_parent.insert(
								relay_parent,
								PerRelayParentData {
									signing_context,
									validator_set,
									..Default::default()
								},
							);
						}
					}

					for relay_parent in deactivated {
						trace!(target: TARGET, "Stop {:?}", relay_parent);
						// defer the cleanup to the view change
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(hash)) => {
					trace!(target: TARGET, "Block finalized {:?}", hash);
				}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					trace!(target: TARGET, "Conclude");
					return Ok(());
				}
			}
		}
	}
}

/// Modify the reputation of a peer based on its behaviour.
async fn modify_reputation<Context>(
	ctx: &mut Context,
	peer: PeerId,
	rep: ReputationChange,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	trace!(target: TARGET, "Reputation change of {:?} for peer {:?}", rep, peer);
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	))
	.await
}

/// Distribute a given valid and signature checked bitfield message.
///
/// For this variant the source is this node.
async fn handle_bitfield_distribution<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	relay_parent: Hash,
	signed_availability: SignedAvailabilityBitfield,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	// Ignore anything the overseer did not tell this subsystem to work on
	let mut job_data = state.per_relay_parent.get_mut(&relay_parent);
	let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
		job_data
	} else {
		trace!(
			target: TARGET,
			"Not supposed to work on relay parent {} related data",
			relay_parent
		);

		return Ok(());
	};
	let validator_set = &job_data.validator_set;
	if validator_set.is_empty() {
		trace!(target: TARGET, "Validator set for {:?} is empty", relay_parent);
		return Ok(());
	}

	let validator_index = signed_availability.validator_index() as usize;
	let validator = if let Some(validator) = validator_set.get(validator_index) {
		validator.clone()
	} else {
		trace!(target: TARGET, "Could not find a validator for index {}", validator_index);
		return Ok(());
	};

	let peer_views = &mut state.peer_views;
	let msg = BitfieldGossipMessage {
		relay_parent,
		signed_availability,
	};

	relay_message(ctx, job_data, peer_views, validator, msg).await?;

	metrics.on_own_bitfield_gossipped();

	Ok(())
}

/// Distribute a given valid and signature checked bitfield message.
///
/// Can be originated by another subsystem or received via network from another peer.
async fn relay_message<Context>(
	ctx: &mut Context,
	job_data: &mut PerRelayParentData,
	peer_views: &mut HashMap<PeerId, View>,
	validator: ValidatorId,
	message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	// notify the overseer about a new and valid signed bitfield
	ctx.send_message(AllMessages::Provisioner(
		ProvisionerMessage::ProvisionableData(ProvisionableData::Bitfield(
			message.relay_parent.clone(),
			message.signed_availability.clone(),
		)),
	))
	.await?;

	let message_sent_to_peer = &mut (job_data.message_sent_to_peer);

	// pass on the bitfield distribution to all interested peers
	let interested_peers = peer_views
		.iter()
		.filter_map(|(peer, view)| {
			// check interest in the peer in this message's relay parent
			if view.contains(&message.relay_parent) {
				// track the message as sent for this peer
				message_sent_to_peer
					.entry(peer.clone())
					.or_default()
					.insert(validator.clone());

				Some(peer.clone())
			} else {
				None
			}
		})
		.collect::<Vec<PeerId>>();

	if interested_peers.is_empty() {
		trace!(
			target: TARGET,
			"No peers are interested in gossip for relay parent {:?}",
			message.relay_parent
		);
	} else {
		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendValidationMessage(
				interested_peers,
				message.into_validation_protocol(),
			),
		))
		.await?;
	}
	Ok(())
}

/// Handle an incoming message from a peer.
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	origin: PeerId,
	message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	// we don't care about this, not part of our view.
	if !state.view.contains(&message.relay_parent) {
		return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
	}

	// Ignore anything the overseer did not tell this subsystem to work on.
	let mut job_data = state.per_relay_parent.get_mut(&message.relay_parent);
	let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
		job_data
	} else {
		return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
	};

	let validator_set = &job_data.validator_set;
	if validator_set.is_empty() {
		trace!(
			target: TARGET,
			"Validator set for relay parent {:?} is empty",
			&message.relay_parent
		);
		return modify_reputation(ctx, origin, COST_MISSING_PEER_SESSION_KEY).await;
	}

	// Use the (untrusted) validator index provided by the signed payload
	// and see if that one actually signed the availability bitset.
	let signing_context = job_data.signing_context.clone();
	let validator_index = message.signed_availability.validator_index() as usize;
	let validator = if let Some(validator) = validator_set.get(validator_index) {
		validator.clone()
	} else {
		return modify_reputation(ctx, origin, COST_VALIDATOR_INDEX_INVALID).await;
	};

	// Check if the peer already sent us a message for the validator denoted in the message earlier.
	// Must be done after validator index verification, in order to avoid storing an unbounded
	// number of set entries.
	let received_set = job_data
		.message_received_from_peer
		.entry(origin.clone())
		.or_default();

	if !received_set.contains(&validator) {
		received_set.insert(validator.clone());
	} else {
		return modify_reputation(ctx, origin, COST_PEER_DUPLICATE_MESSAGE).await;
	};

	if message
		.signed_availability
		.check_signature(&signing_context, &validator)
		.is_ok()
	{
		metrics.on_bitfield_received();
		let one_per_validator = &mut (job_data.one_per_validator);

		// only relay_message a message of a validator once
		if one_per_validator.get(&validator).is_some() {
			trace!(
				target: TARGET,
				"Already received a message for validator at index {}",
				validator_index
			);
			modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE).await?;
			return Ok(());
		}
		one_per_validator.insert(validator.clone(), message.clone());

		relay_message(ctx, job_data, &mut state.peer_views, validator, message).await?;

		modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE_FIRST).await
	} else {
		modify_reputation(ctx, origin, COST_SIGNATURE_INVALID).await
	}
}

/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	metrics: &Metrics,
	bridge_message: NetworkBridgeEvent<protocol_v1::BitfieldDistributionMessage>,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	match bridge_message {
		NetworkBridgeEvent::PeerConnected(peerid, _role) => {
			// insert if none already present
			state.peer_views.entry(peerid).or_default();
		}
		NetworkBridgeEvent::PeerDisconnected(peerid) => {
			// get rid of superfluous data
			state.peer_views.remove(&peerid);
		}
		NetworkBridgeEvent::PeerViewChange(peerid, view) => {
			handle_peer_view_change(ctx, state, peerid, view).await?;
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			handle_our_view_change(state, view)?;
		}
		NetworkBridgeEvent::PeerMessage(remote, message) => {
			match message {
				protocol_v1::BitfieldDistributionMessage::Bitfield(relay_parent, bitfield) => {
					trace!(target: TARGET, "Received bitfield gossip from peer {:?}", &remote);
					let gossiped_bitfield = BitfieldGossipMessage {
						relay_parent,
						signed_availability: bitfield,
					};
					process_incoming_peer_message(ctx, state, metrics, remote, gossiped_bitfield).await?;
				}
			}
		}
	}
	Ok(())
}

/// Handle the changes necassary when our view changes.
fn handle_our_view_change(state: &mut ProtocolState, view: View) -> SubsystemResult<()> {
	let old_view = std::mem::replace(&mut (state.view), view);

	for added in state.view.difference(&old_view) {
		if !state.per_relay_parent.contains_key(&added) {
			warn!(
				target: TARGET,
				"Our view contains {} but the overseer never told use we should work on this",
				&added
			);
		}
	}
	for removed in old_view.difference(&state.view) {
		// cleanup relay parents we are not interested in any more
		let _ = state.per_relay_parent.remove(&removed);
	}
	Ok(())
}


// Send the difference between two views which were not sent
// to that particular peer.
async fn handle_peer_view_change<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	view: View,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	let current = state.peer_views.entry(origin.clone()).or_default();

	let added: Vec<Hash> = view.difference(&*current).cloned().collect();

	*current = view;

	// Send all messages we've seen before and the peer is now interested
	// in to that peer.

	let delta_set: Vec<(ValidatorId, BitfieldGossipMessage)> = added
		.into_iter()
		.filter_map(|new_relay_parent_interest| {
			if let Some(job_data) = (&*state).per_relay_parent.get(&new_relay_parent_interest) {
				// Send all jointly known messages for a validator (given the current relay parent)
				// to the peer `origin`...
				let one_per_validator = job_data.one_per_validator.clone();
				let origin = origin.clone();
				Some(
					one_per_validator
						.into_iter()
						.filter(move |(validator, _message)| {
							// ..except for the ones the peer already has.
							job_data.message_from_validator_needed_by_peer(&origin, validator)
						}),
				)
			} else {
				// A relay parent is in the peers view, which is not in ours, ignore those.
				None
			}
		})
		.flatten()
		.collect();

	for (validator, message) in delta_set.into_iter() {
		send_tracked_gossip_message(ctx, state, origin.clone(), validator, message).await?;
	}

	Ok(())
}

/// Send a gossip message and track it in the per relay parent data.
async fn send_tracked_gossip_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	dest: PeerId,
	validator: ValidatorId,
	message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	let job_data = if let Some(job_data) = state.per_relay_parent.get_mut(&message.relay_parent) {
		job_data
	} else {
		return Ok(());
	};

	let message_sent_to_peer = &mut (job_data.message_sent_to_peer);
	message_sent_to_peer
		.entry(dest.clone())
		.or_default()
		.insert(validator.clone());

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendValidationMessage(
			vec![dest],
			message.into_validation_protocol(),
		),
	))
	.await?;

	Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
	C: SubsystemContext<Message = BitfieldDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		SpawnedSubsystem {
			name: "bitfield-distribution-subsystem",
			future: Box::pin(async move { Self::run(self, ctx) }.map(|_| ())),
		}
	}
}

/// Query our validator set and signing context for a particular relay parent.
async fn query_basics<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> SubsystemResult<Option<(Vec<ValidatorId>, SigningContext)>>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	let (validators_tx, validators_rx) = oneshot::channel();
	let (session_tx, session_rx) = oneshot::channel();

	let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent.clone(),
		RuntimeApiRequest::Validators(validators_tx),
	));

	let query_signing = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent.clone(),
		RuntimeApiRequest::SessionIndexForChild(session_tx),
	));

	ctx.send_messages(std::iter::once(query_validators).chain(std::iter::once(query_signing)))
		.await?;

	match (validators_rx.await?, session_rx.await?) {
		(Ok(v), Ok(s)) => Ok(Some((
			v,
			SigningContext { parent_hash: relay_parent, session_index: s },
		))),
		(Err(e), _) | (_, Err(e)) => {
			warn!(target: TARGET, "Failed to fetch basics from runtime API: {:?}", e);
			Ok(None)
		}
	}
}

#[derive(Clone)]
struct MetricsInner {
	gossipped_own_availability_bitfields: prometheus::Counter<prometheus::U64>,
	received_availability_bitfields: prometheus::Counter<prometheus::U64>,
}

/// Bitfield Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_own_bitfield_gossipped(&self) {
		if let Some(metrics) = &self.0 {
			metrics.gossipped_own_availability_bitfields.inc();
		}
	}

	fn on_bitfield_received(&self) {
		if let Some(metrics) = &self.0 {
			metrics.received_availability_bitfields.inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			gossipped_own_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"parachain_gossipped_own_availabilty_bitfields_total",
					"Number of own availability bitfields sent to other peers."
				)?,
				registry,
			)?,
			received_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"parachain_received_availabilty_bitfields_total",
					"Number of valid availability bitfields received from other peers."
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}


#[cfg(test)]
mod test {
	use super::*;
	use bitvec::bitvec;
	use futures::executor;
	use maplit::hashmap;
	use polkadot_primitives::v1::{Signed, ValidatorPair, AvailabilityBitfield};
	use polkadot_node_subsystem_test_helpers::make_subsystem_context;
	use polkadot_node_subsystem_util::TimeoutExt;
	use sp_core::crypto::Pair;
	use std::time::Duration;
	use assert_matches::assert_matches;
	use polkadot_node_network_protocol::ObservedRole;

	macro_rules! view {
		( $( $hash:expr ),* $(,)? ) => [
			View(vec![ $( $hash.clone() ),* ])
		];
	}

	macro_rules! peers {
		( $( $peer:expr ),* $(,)? ) => [
			vec![ $( $peer.clone() ),* ]
		];
	}

	macro_rules! launch {
		($fut:expr) => {
			$fut
			.timeout(Duration::from_millis(10))
			.await
			.expect("10ms is more than enough for sending messages.")
			.expect("Error values should really never occur.")
		};
	}

	/// A very limited state, only interested in the relay parent of the
	/// given message, which must be signed by `validator` and a set of peers
	/// which are also only interested in that relay parent.
	fn prewarmed_state(
		validator: ValidatorId,
		signing_context: SigningContext,
		known_message: BitfieldGossipMessage,
		peers: Vec<PeerId>,
	) -> ProtocolState {
		let relay_parent = known_message.relay_parent.clone();
		ProtocolState {
			per_relay_parent: hashmap! {
				relay_parent.clone() =>
					PerRelayParentData {
						signing_context,
						validator_set: vec![validator.clone()],
						one_per_validator: hashmap! {
							validator.clone() => known_message.clone(),
						},
						message_received_from_peer: hashmap!{},
						message_sent_to_peer: hashmap!{},
					},
			},
			peer_views: peers
				.into_iter()
				.map(|peer| (peer, view!(relay_parent)))
				.collect(),
			view: view!(relay_parent),
		}
	}

	fn state_with_view(view: View, relay_parent: Hash) -> (ProtocolState, SigningContext, ValidatorPair) {
		let mut state = ProtocolState::default();

		let (validator_pair, _seed) = ValidatorPair::generate();
		let validator = validator_pair.public();

		let signing_context = SigningContext {
			session_index: 1,
			parent_hash: relay_parent.clone(),
		};

		state.per_relay_parent = view.0.iter().map(|relay_parent| {(
				relay_parent.clone(),
				PerRelayParentData {
					signing_context: signing_context.clone(),
					validator_set: vec![validator.clone()],
					one_per_validator: hashmap!{},
					message_received_from_peer: hashmap!{},
					message_sent_to_peer: hashmap!{},
				})
			}).collect();

		state.view = view;

		(state, signing_context, validator_pair)
	}

	#[test]
	fn receive_invalid_signature() {
		let _ = env_logger::builder()
			.filter(None, log::LevelFilter::Trace)
			.is_test(true)
			.try_init();

		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		assert_ne!(peer_a, peer_b);

		let signing_context = SigningContext {
			session_index: 1,
			parent_hash: hash_a.clone(),
		};

		// validator 0 key pair
		let (validator_pair, _seed) = ValidatorPair::generate();
		let validator = validator_pair.public();

		// another validator not part of the validatorset
		let (mallicious, _seed) = ValidatorPair::generate();

		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &mallicious);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		let mut state = prewarmed_state(
			validator.clone(),
			signing_context.clone(),
			msg.clone(),
			vec![peer_b.clone()],
		);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(peer_b.clone(), msg.into_network_message()),
			));

			// reputation change due to invalid validator index
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_SIGNATURE_INVALID)
				}
			);
		});
	}

	#[test]
	fn receive_invalid_validator_index() {
		let _ = env_logger::builder()
			.filter(None, log::LevelFilter::Trace)
			.is_test(true)
			.try_init();

		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into(); // other

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		assert_ne!(peer_a, peer_b);

		// validator 0 key pair
		let (mut state, signing_context, validator_pair) =
			state_with_view(view![hash_a, hash_b], hash_a.clone());

		state.peer_views.insert(peer_b.clone(), view![hash_a]);

		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 42, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(peer_b.clone(), msg.into_network_message()),
			));

			// reputation change due to invalid validator index
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_VALIDATOR_INDEX_INVALID)
				}
			);
		});
	}

	#[test]
	fn receive_duplicate_messages() {
		let _ = env_logger::builder()
			.filter(None, log::LevelFilter::Trace)
			.is_test(true)
			.try_init();

		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		assert_ne!(peer_a, peer_b);

		// validator 0 key pair
		let (mut state, signing_context, validator_pair) =
			state_with_view(view![hash_a, hash_b], hash_a.clone());

		// create a signed message by validator 0
		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed_bitfield =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed_bitfield.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			// send a first message
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// none of our peers has any interest in any messages
			// so we do not receive a network send type message here
			// but only the one for the next subsystem
			assert_matches!(
				handle.recv().await,
				AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
					ProvisionableData::Bitfield(hash, signed)
				)) => {
					assert_eq!(hash, hash_a);
					assert_eq!(signed, signed_bitfield)
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST)
				}
			);

			// let peer A send the same message again
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					msg.clone().into_network_message(),
				),
			));

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE)
				}
			);

			// let peer B send the initial message again
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE)
				}
			);
		});
	}

	#[test]
	fn changing_view() {
		let _ = env_logger::builder()
			.filter(None, log::LevelFilter::Trace)
			.is_test(true)
			.try_init();

		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		assert_ne!(peer_a, peer_b);

		// validator 0 key pair
		let (mut state, signing_context, validator_pair) = state_with_view(view![hash_a, hash_b], hash_a.clone());

		// create a signed message by validator 0
		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed_bitfield =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed_bitfield.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full),
			));

			// make peer b interested
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a, hash_b]),
			));

			assert!(state.peer_views.contains_key(&peer_b));

			// recv a first message from the network
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// gossip to the overseer
			assert_matches!(
				handle.recv().await,
				AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
					ProvisionableData::Bitfield(hash, signed)
				)) => {
					assert_eq!(hash, hash_a);
					assert_eq!(signed, signed_bitfield)
				}
			);

			// gossip to the network
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage (
					peers, out_msg,
				)) => {
					assert_eq!(peers, peers![peer_b]);
					assert_eq!(out_msg, msg.clone().into_validation_protocol());
				}
			);

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST)
				}
			);

			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![]),
			));

			assert!(state.peer_views.contains_key(&peer_b));
			assert_eq!(
				state.peer_views.get(&peer_b).expect("Must contain value for peer B"),
				&view![]
			);

			// on rx of the same message, since we are not interested,
			// should give penalty
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE)
				}
			);

			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerDisconnected(peer_b.clone()),
			));

			// we are not interested in any peers at all anymore
			state.view = view![];

			// on rx of the same message, since we are not interested,
			// should give penalty
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					msg.clone().into_network_message(),
				),
			));

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_NOT_IN_VIEW)
				}
			);

		});
	}
}
