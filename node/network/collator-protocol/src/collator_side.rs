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

use std::collections::{HashMap, HashSet};

use futures::{FutureExt, channel::oneshot};
use sp_core::Pair;

use polkadot_primitives::v1::{AuthorityDiscoveryId, CandidateHash, CandidateReceipt, CollatorPair, CoreIndex, CoreState, GroupIndex, Hash, Id as ParaId};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, PerLeafSpan, SubsystemContext, jaeger,
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeEvent, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	OurView, PeerId, View, peer_set::PeerSet,
	request_response::{
		IncomingRequest,
		v1::{CollationFetchingRequest, CollationFetchingResponse},
	},
	v1 as protocol_v1,
	UnifiedReputationChange as Rep,
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
	runtime::{RuntimeInfo, get_availability_cores, get_group_rotation_info}
};
use polkadot_node_primitives::{SignedFullStatement, Statement, PoV};

use crate::error::{Fatal, NonFatal, log_error};
use super::{LOG_TARGET,  Result};

const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("An unexpected message");

#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_advertisment_made(&self) {
		if let Some(metrics) = &self.0 {
			metrics.advertisements_made.inc();
		}
	}

	fn on_collation_sent(&self) {
		if let Some(metrics) = &self.0 {
			metrics.collations_sent.inc();
		}
	}

	/// Provide a timer for `process_msg` which observes on drop.
	fn time_process_msg(&self) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_msg.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	advertisements_made: prometheus::Counter<prometheus::U64>,
	collations_sent: prometheus::Counter<prometheus::U64>,
	process_msg: prometheus::Histogram,
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry)
		-> std::result::Result<Self, prometheus::PrometheusError>
	{
		let metrics = MetricsInner {
			advertisements_made: prometheus::register(
				prometheus::Counter::new(
					"parachain_collation_advertisements_made_total",
					"A number of collation advertisements sent to validators.",
				)?,
				registry,
			)?,
			collations_sent: prometheus::register(
				prometheus::Counter::new(
					"parachain_collations_sent_total",
					"A number of collations sent to validators.",
				)?,
				registry,
			)?,
			process_msg: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_collator_protocol_collator_process_msg",
						"Time spent within `collator_protocol_collator::process_msg`",
					)
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}

/// The group of validators that is assigned to our para at a given point of time.
///
/// This structure is responsible for keeping track of which validators belong to a certain group for a para. It also
/// stores a mapping from [`PeerId`] to [`ValidatorId`] as we learn about it over the lifetime of this object. Besides
/// that it also keeps track to which validators we advertised our collation.
#[derive(Debug)]
struct ValidatorGroup {
	/// All [`AuthorityDiscoveryId`]'s that are assigned to us in this group.
	discovery_ids: HashSet<AuthorityDiscoveryId>,
	/// All [`ValidatorId`]'s of the current group to that we advertised our collation.
	advertised_to: HashSet<AuthorityDiscoveryId>,
}

impl ValidatorGroup {
	/// Returns `true` if we should advertise our collation to the given peer.
	fn should_advertise_to(&self, peer_ids: &HashMap<PeerId, AuthorityDiscoveryId>, peer: &PeerId)
		-> bool {
		match peer_ids.get(peer) {
			Some(discovery_id) => !self.advertised_to.contains(discovery_id),
			None => false,
		}
	}

	/// Should be called after we advertised our collation to the given `peer` to keep track of it.
	fn advertised_to_peer(&mut self, peer_ids: &HashMap<PeerId, AuthorityDiscoveryId>, peer: &PeerId) {
		if let Some(validator_id) = peer_ids.get(peer) {
			self.advertised_to.insert(validator_id.clone());
		}
	}
}

impl From<HashSet<AuthorityDiscoveryId>> for ValidatorGroup {
	fn from(discovery_ids: HashSet<AuthorityDiscoveryId>) -> Self {
		Self {
			discovery_ids,
			advertised_to: HashSet::new(),
		}
	}
}

/// The status of a collation as seen from the collator.
enum CollationStatus {
	/// The collation was created, but we did not advertise it to any validator.
	Created,
	/// The collation was advertised to at least one validator.
	Advertised,
	/// The collation was requested by at least one validator.
	Requested,
}

impl CollationStatus {
	/// Advance to the [`Self::Advertised`] status.
	///
	/// This ensures that `self` isn't already [`Self::Requested`].
	fn advance_to_advertised(&mut self) {
		if !matches!(self, Self::Requested) {
			*self = Self::Advertised;
		}
	}

	/// Advance to the [`Self::Requested`] status.
	fn advance_to_requested(&mut self) {
		*self = Self::Requested;
	}
}

/// A collation built by the collator.
struct Collation {
	receipt: CandidateReceipt,
	pov: PoV,
	status: CollationStatus,
}

struct State {
	/// Our network peer id.
	local_peer_id: PeerId,

	/// Our collator pair.
	collator_pair: CollatorPair,

	/// The para this collator is collating on.
	/// Starts as `None` and is updated with every `CollateOn` message.
	collating_on: Option<ParaId>,

	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our own view.
	view: OurView,

	/// Span per relay parent.
	span_per_relay_parent: HashMap<Hash, PerLeafSpan>,

	/// Possessed collations.
	///
	/// We will keep up to one local collation per relay-parent.
	collations: HashMap<Hash, Collation>,

	/// The result senders per collation.
	collation_result_senders: HashMap<CandidateHash, oneshot::Sender<SignedFullStatement>>,

	/// Our validator groups per active leaf.
	our_validators_groups: HashMap<Hash, ValidatorGroup>,

	/// The mapping from [`PeerId`] to [`ValidatorId`]. This is filled over time as we learn the [`PeerId`]'s by `PeerConnected` events.
	peer_ids: HashMap<PeerId, AuthorityDiscoveryId>,

	/// Metrics.
	metrics: Metrics,
}

impl State {
	/// Creates a new `State` instance with the given parameters and setting all remaining
	/// state fields to their default values (i.e. empty).
	fn new(local_peer_id: PeerId, collator_pair: CollatorPair, metrics: Metrics) -> State {
		State {
			local_peer_id,
			collator_pair,
			metrics,
			collating_on: Default::default(),
			peer_views: Default::default(),
			view: Default::default(),
			span_per_relay_parent: Default::default(),
			collations: Default::default(),
			collation_result_senders: Default::default(),
			our_validators_groups: Default::default(),
			peer_ids: Default::default(),
		}
	}

	/// Get all peers which have the given relay parent in their view.
	fn peers_interested_in_leaf(&self, relay_parent: &Hash) -> Vec<PeerId> {
		self.peer_views
			.iter()
			.filter(|(_, v)| v.contains(relay_parent))
			.map(|(peer, _)| *peer)
			.collect()
	}
}

/// Distribute a collation.
///
/// Figure out the core our para is assigned to and the relevant validators.
/// Issue a connection request to these validators.
/// If the para is not scheduled or next up on any core, at the relay-parent,
/// or the relay-parent isn't in the active-leaves set, we ignore the message
/// as it must be invalid in that case - although this indicates a logic error
/// elsewhere in the node.
#[tracing::instrument(level = "trace", skip(ctx, runtime, state, pov), fields(subsystem = LOG_TARGET))]
async fn distribute_collation(
	ctx: &mut impl SubsystemContext,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	id: ParaId,
	receipt: CandidateReceipt,
	pov: PoV,
	result_sender: Option<oneshot::Sender<SignedFullStatement>>,
) -> Result<()> {
	let relay_parent = receipt.descriptor.relay_parent;

	// This collation is not in the active-leaves set.
	if !state.view.contains(&relay_parent) {
		tracing::warn!(
			target: LOG_TARGET,
			?relay_parent,
			"distribute collation message parent is outside of our view",
		);

		return Ok(());
	}

	// We have already seen collation for this relay parent.
	if state.collations.contains_key(&relay_parent) {
		return Ok(());
	}

	// Determine which core the para collated-on is assigned to.
	// If it is not scheduled then ignore the message.
	let (our_core, num_cores) = match determine_core(ctx, id, relay_parent).await? {
		Some(core) => core,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				para_id = %id,
				?relay_parent,
				"looks like no core is assigned to {} at {}", id, relay_parent,
			);

			return Ok(())
		}
	};

	// Determine the group on that core and the next group on that core.
	let (current_validators, next_validators) =
		determine_our_validators(ctx, runtime, our_core, num_cores, relay_parent,).await?;

	if current_validators.validators.is_empty() && next_validators.validators.is_empty() {
		tracing::warn!(
			target: LOG_TARGET,
			core = ?our_core,
			"there are no validators assigned to core",
		);

		return Ok(());
	}

	tracing::debug!(
		target: LOG_TARGET,
		para_id = %id,
		relay_parent = %relay_parent,
		candidate_hash = ?receipt.hash(),
		pov_hash = ?pov.hash(),
		core = ?our_core,
		?current_validators,
		?next_validators,
		"Accepted collation, connecting to validators."
	);

	let validator_group: HashSet<_> = current_validators.validators.iter().map(Clone::clone).collect();

	// Issue a discovery request for the validators of the current group and the next group:
	connect_to_validators(
		ctx,
		current_validators.validators
			.into_iter()
			.chain(next_validators.validators.into_iter())
			.collect(),
	).await;

	state.our_validators_groups.insert(relay_parent, validator_group.into());

	if let Some(result_sender) = result_sender {
		state.collation_result_senders.insert(receipt.hash(), result_sender);
	}

	state.collations.insert(relay_parent, Collation { receipt, pov, status: CollationStatus::Created });

	// Make sure already connected peers get collations:
	for peer_id in state.peers_interested_in_leaf(&relay_parent) {
		advertise_collation(ctx, state, relay_parent, peer_id).await;
	}

	Ok(())
}

/// Get the Id of the Core that is assigned to the para being collated on if any
/// and the total number of cores.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn determine_core(
	ctx: &mut impl SubsystemContext,
	para_id: ParaId,
	relay_parent: Hash,
) -> Result<Option<(CoreIndex, usize)>> {
	let cores = get_availability_cores(ctx, relay_parent).await?;

	for (idx, core) in cores.iter().enumerate() {
		if let CoreState::Scheduled(occupied) = core {
			if occupied.para_id == para_id {
				return Ok(Some(((idx as u32).into(), cores.len())));
			}
		}
	}
	Ok(None)
}

/// Validators of a particular group index.
#[derive(Debug)]
struct GroupValidators {
	/// The group those validators belong to.
	group: GroupIndex,
	/// The validators of above group (their discovery keys).
	validators: Vec<AuthorityDiscoveryId>,
}

/// Figure out current and next group of validators assigned to the para being collated on.
///
/// Returns [`ValidatorId`]'s of current and next group as determined based on the `relay_parent`.
#[tracing::instrument(level = "trace", skip(ctx, runtime), fields(subsystem = LOG_TARGET))]
async fn determine_our_validators(
	ctx: &mut impl SubsystemContext,
	runtime: &mut RuntimeInfo,
	core_index: CoreIndex,
	cores: usize,
	relay_parent: Hash,
) -> Result<(GroupValidators, GroupValidators)> {
	let info = &runtime.get_session_info(ctx, relay_parent).await?.session_info;
	tracing::debug!(target: LOG_TARGET, ?info, "Received info");
	let groups = &info.validator_groups;
	let rotation_info = get_group_rotation_info(ctx, relay_parent).await?;

	let current_group_index = rotation_info.group_for_core(core_index, cores);
	let current_validators = groups.get(current_group_index.0 as usize).map(|v| v.as_slice()).unwrap_or_default();

	let next_group_idx = (current_group_index.0 as usize + 1) % groups.len();
	let next_validators = groups.get(next_group_idx).map(|v| v.as_slice()).unwrap_or_default();

	let validators = &info.discovery_keys;

	let current_validators = current_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();
	let next_validators = next_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();

	let current_validators = GroupValidators {
		group: current_group_index,
		validators: current_validators,
	};
	let next_validators = GroupValidators {
		group: GroupIndex(next_group_idx as u32),
		validators: next_validators,
	};

	Ok((current_validators, next_validators))
}

/// Issue a `Declare` collation message to the given `peer`.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn declare(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	peer: PeerId,
) {
	let declare_signature_payload = protocol_v1::declare_signature_payload(&state.local_peer_id);

	if let Some(para_id) = state.collating_on {
		let wire_message = protocol_v1::CollatorProtocolMessage::Declare(
			state.collator_pair.public(),
			para_id,
			state.collator_pair.sign(&declare_signature_payload),
		);

		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendCollationMessage(
				vec![peer],
				protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
			)
		)).await;
	}
}

/// Issue a connection request to a set of validators and
/// revoke the previous connection request.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn connect_to_validators(
	ctx: &mut impl SubsystemContext,
	validator_ids: Vec<AuthorityDiscoveryId>,
)  {
	// ignore address resolution failure
	// will reissue a new request on new collation
	let (failed, _) = oneshot::channel();
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
		validator_ids, peer_set: PeerSet::Collation, failed,
	})).await;
}

/// Advertise collation to the given `peer`.
///
/// This will only advertise a collation if there exists one for the given `relay_parent` and the given `peer` is
/// set as validator for our para at the given `relay_parent`.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn advertise_collation(
	ctx: &mut impl SubsystemContext,
	state: &mut State,
	relay_parent: Hash,
	peer: PeerId,
) {
	let should_advertise = state.our_validators_groups
		.get(&relay_parent)
		.map(|g| g.should_advertise_to(&state.peer_ids, &peer))
		.unwrap_or(false);

	match (state.collations.get_mut(&relay_parent), should_advertise) {
		(None, _) => {
			tracing::trace!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"No collation to advertise.",
			);
			return
		},
		(_, false) => {
			tracing::debug!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"Not advertising collation as we already advertised it to this validator.",
			);
			return
		}
		(Some(collation), true) => {
			tracing::debug!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"Advertising collation.",
			);
			collation.status.advance_to_advertised()
		},
	}

	let wire_message = protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
		relay_parent,
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendCollationMessage(
			vec![peer.clone()],
			protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
		)
	)).await;

	if let Some(validators) = state.our_validators_groups.get_mut(&relay_parent) {
		validators.advertised_to_peer(&state.peer_ids, &peer);
	}

	state.metrics.on_advertisment_made();
}

/// The main incoming message dispatching switch.
#[tracing::instrument(level = "trace", skip(ctx, runtime, state), fields(subsystem = LOG_TARGET))]
async fn process_msg(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	msg: CollatorProtocolMessage,
) -> Result<()> {
	use CollatorProtocolMessage::*;

	let _timer = state.metrics.time_process_msg();

	match msg {
		CollateOn(id) => {
			state.collating_on = Some(id);
		}
		DistributeCollation(receipt, pov, result_sender) => {
			let _span1 = state.span_per_relay_parent
				.get(&receipt.descriptor.relay_parent).map(|s| s.child("distributing-collation"));
			let _span2 = jaeger::Span::new(&pov, "distributing-collation");
			match state.collating_on {
				Some(id) if receipt.descriptor.para_id != id => {
					// If the ParaId of a collation requested to be distributed does not match
					// the one we expect, we ignore the message.
					tracing::warn!(
						target: LOG_TARGET,
						para_id = %receipt.descriptor.para_id,
						collating_on = %id,
						"DistributeCollation for unexpected para_id",
					);
				}
				Some(id) => {
					distribute_collation(ctx, runtime, state, id, receipt, pov, result_sender).await?;
				}
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						para_id = %receipt.descriptor.para_id,
						"DistributeCollation message while not collating on any",
					);
				}
			}
		}
		FetchCollation(_, _, _, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"FetchCollation message is not expected on the collator side of the protocol",
			);
		}
		ReportCollator(_) => {
			tracing::warn!(
				target: LOG_TARGET,
				"ReportCollator message is not expected on the collator side of the protocol",
			);
		}
		NoteGoodCollation(_) => {
			tracing::warn!(
				target: LOG_TARGET,
				"NoteGoodCollation message is not expected on the collator side of the protocol",
			);
		}
		NotifyCollationSeconded(_, _, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"NotifyCollationSeconded message is not expected on the collator side of the protocol",
			);
		}
		NetworkBridgeUpdateV1(event) => {
			if let Err(e) = handle_network_msg(
				ctx,
				runtime,
				state,
				event,
			).await {
				tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to handle incoming network message",
				);
			}
		},
		CollationFetchingRequest(incoming) => {
			let _span = state.span_per_relay_parent.get(&incoming.payload.relay_parent).map(|s| s.child("request-collation"));
			match state.collating_on {
				Some(our_para_id) => {
					if our_para_id == incoming.payload.para_id {
						let (receipt, pov) = if let Some(collation) = state.collations.get_mut(&incoming.payload.relay_parent) {
							collation.status.advance_to_requested();
							(collation.receipt.clone(), collation.pov.clone())
						} else {
							tracing::warn!(
								target: LOG_TARGET,
								relay_parent = %incoming.payload.relay_parent,
								"received a `RequestCollation` for a relay parent we don't have collation stored.",
							);

							return Ok(());
						};

						let _span = _span.as_ref().map(|s| s.child("sending"));
						send_collation(state, incoming, receipt, pov).await;
					} else {
						tracing::warn!(
							target: LOG_TARGET,
							for_para_id = %incoming.payload.para_id,
							our_para_id = %our_para_id,
							"received a `CollationFetchingRequest` for unexpected para_id",
						);
					}
				}
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						for_para_id = %incoming.payload.para_id,
						"received a `RequestCollation` while not collating on any para",
					);
				}
			}
		}
	}

	Ok(())
}

/// Issue a response to a previously requested collation.
#[tracing::instrument(level = "trace", skip(state, pov), fields(subsystem = LOG_TARGET))]
async fn send_collation(
	state: &mut State,
	request: IncomingRequest<CollationFetchingRequest>,
	receipt: CandidateReceipt,
	pov: PoV,
) {
	if let Err(_) = request.send_response(CollationFetchingResponse::Collation(receipt, pov)) {
		tracing::warn!(
			target: LOG_TARGET,
			"Sending collation response failed",
		);
	}
	state.metrics.on_collation_sent();
}

/// A networking messages switch.
#[tracing::instrument(level = "trace", skip(ctx, runtime, state), fields(subsystem = LOG_TARGET))]
async fn handle_incoming_peer_message(
	ctx: &mut impl SubsystemContext,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
) -> Result<()> {
	use protocol_v1::CollatorProtocolMessage::*;

	match msg {
		Declare(_, _, _) => {
			tracing::trace!(
				target: LOG_TARGET,
				?origin,
				"Declare message is not expected on the collator side of the protocol",
			);

			// If we are declared to, this is another collator, and we should disconnect.
			ctx.send_message(
				NetworkBridgeMessage::DisconnectPeer(origin, PeerSet::Collation).into()
			).await;
		}
		AdvertiseCollation(_) => {
			tracing::trace!(
				target: LOG_TARGET,
				?origin,
				"AdvertiseCollation message is not expected on the collator side of the protocol",
			);

			ctx.send_message(
				NetworkBridgeMessage::ReportPeer(origin.clone(), COST_UNEXPECTED_MESSAGE).into()
			).await;

			// If we are advertised to, this is another collator, and we should disconnect.
			ctx.send_message(
				NetworkBridgeMessage::DisconnectPeer(origin, PeerSet::Collation).into()
			).await;
		}
		CollationSeconded(relay_parent, statement) => {
			if !matches!(statement.unchecked_payload(), Statement::Seconded(_)) {
				tracing::warn!(
					target: LOG_TARGET,
					?statement,
					?origin,
					"Collation seconded message received with none-seconded statement.",
				);
			} else {
				let statement = runtime.check_signature(ctx, relay_parent, statement)
					.await?
					.map_err(NonFatal::InvalidStatementSignature)?;

				let removed = state.collation_result_senders
					.remove(&statement.payload().candidate_hash());

				if let Some(sender) = removed {
					tracing::trace!(
						target: LOG_TARGET,
						?statement,
						?origin,
						"received a `CollationSeconded`",
					);
					let _ = sender.send(statement);
				}
			}
		}
	}

	Ok(())
}

/// Our view has changed.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_peer_view_change(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	peer_id: PeerId,
	view: View,
) {
	let current = state.peer_views.entry(peer_id.clone()).or_default();

	let added: Vec<Hash> = view.difference(&*current).cloned().collect();

	*current = view;

	for added in added.into_iter() {
		advertise_collation(ctx, state, added, peer_id.clone()).await;
	}
}

/// Bridge messages switch.
#[tracing::instrument(level = "trace", skip(ctx, runtime, state), fields(subsystem = LOG_TARGET))]
async fn handle_network_msg(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	bridge_message: NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>,
) -> Result<()> {
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(peer_id, observed_role, maybe_authority) => {
			// If it is possible that a disconnected validator would attempt a reconnect
			// it should be handled here.
			tracing::trace!(
				target: LOG_TARGET,
				?peer_id,
				?observed_role,
				"Peer connected",
			);
			if let Some(authority) = maybe_authority {
				tracing::trace!(
					target: LOG_TARGET,
					?authority,
					?peer_id,
					"Connected to requested validator"
				);
				state.peer_ids.insert(peer_id, authority);

				declare(ctx, state, peer_id).await;
			}
		}
		PeerViewChange(peer_id, view) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer_id,
				?view,
				"Peer view change",
			);
			handle_peer_view_change(ctx, state, peer_id, view).await;
		}
		PeerDisconnected(peer_id) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer_id,
				"Peer disconnected",
			);
			state.peer_views.remove(&peer_id);
			state.peer_ids.remove(&peer_id);
		}
		OurViewChange(view) => {
			tracing::trace!(
				target: LOG_TARGET,
				?view,
				"Own view change",
			);
			handle_our_view_change(state, view).await?;
		}
		PeerMessage(remote, msg) => {
			handle_incoming_peer_message(ctx, runtime, state, remote, msg).await?;
		}
	}

	Ok(())
}

/// Handles our view changes.
#[tracing::instrument(level = "trace", skip(state), fields(subsystem = LOG_TARGET))]
async fn handle_our_view_change(
	state: &mut State,
	view: OurView,
) -> Result<()> {
	for removed in state.view.difference(&view) {
		tracing::debug!(target: LOG_TARGET, relay_parent = ?removed, "Removing relay parent because our view changed.");

		if let Some(collation) = state.collations.remove(removed) {
			state.collation_result_senders.remove(&collation.receipt.hash());

			match collation.status {
				CollationStatus::Created => tracing::warn!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation wasn't advertised to any validator.",
				),
				CollationStatus::Advertised => tracing::debug!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation was advertised but not requested by any validator.",
				),
				CollationStatus::Requested => tracing::debug!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation was requested.",
				)
			}
		}
		state.our_validators_groups.remove(removed);
		state.span_per_relay_parent.remove(removed);
	}

	state.view = view;

	Ok(())
}

/// The collator protocol collator side main loop.
#[tracing::instrument(skip(ctx, collator_pair, metrics), fields(subsystem = LOG_TARGET))]
pub(crate) async fn run(
	mut ctx: impl SubsystemContext<Message = CollatorProtocolMessage>,
	local_peer_id: PeerId,
	collator_pair: CollatorPair,
	metrics: Metrics,
) -> Result<()> {
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State::new(local_peer_id, collator_pair, metrics);
	let mut runtime = RuntimeInfo::new(None);

	loop {
		let msg = ctx.recv().fuse().await.map_err(Fatal::SubsystemReceive)?;
		match msg {
			Communication { msg } => {
				log_error(
					process_msg(&mut ctx, &mut runtime, &mut state, msg).await,
					"Failed to process message"
				)?;
			},
			Signal(ActiveLeaves(_update)) => {}
			Signal(BlockFinalized(..)) => {}
			Signal(Conclude) => return Ok(()),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::{sync::Arc, time::Duration};

	use assert_matches::assert_matches;
	use futures::{executor, future, Future};

	use sp_core::{crypto::Pair, Decode};
	use sp_keyring::Sr25519Keyring;
	use sp_runtime::traits::AppVerify;

	use polkadot_node_network_protocol::{
		our_view,
		view,
		request_response::request::IncomingRequest,
	};
	use polkadot_node_subsystem_util::TimeoutExt;
	use polkadot_primitives::v1::{AuthorityDiscoveryId, CandidateDescriptor, CollatorPair, GroupRotationInfo, ScheduledCore, SessionIndex, SessionInfo, ValidatorId, ValidatorIndex};
	use polkadot_node_primitives::BlockData;
	use polkadot_subsystem::{
		jaeger,
		messages::{RuntimeApiMessage, RuntimeApiRequest},
		ActiveLeavesUpdate, ActivatedLeaf, LeafStatus,
	};
	use polkadot_subsystem_testhelpers as test_helpers;

	#[derive(Default)]
	struct TestCandidateBuilder {
		para_id: ParaId,
		pov_hash: Hash,
		relay_parent: Hash,
		commitments_hash: Hash,
	}

	impl TestCandidateBuilder {
		fn build(self) -> CandidateReceipt {
			CandidateReceipt {
				descriptor: CandidateDescriptor {
					para_id: self.para_id,
					pov_hash: self.pov_hash,
					relay_parent: self.relay_parent,
					..Default::default()
				},
				commitments_hash: self.commitments_hash,
			}
		}
	}

	#[derive(Clone)]
	struct TestState {
		para_id: ParaId,
		validators: Vec<Sr25519Keyring>,
		session_info: SessionInfo,
		group_rotation_info: GroupRotationInfo,
		validator_peer_id: Vec<PeerId>,
		relay_parent: Hash,
		availability_core: CoreState,
		local_peer_id: PeerId,
		collator_pair: CollatorPair,
		session_index: SessionIndex,
	}

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	fn validator_authority_id(val_ids: &[Sr25519Keyring]) -> Vec<AuthorityDiscoveryId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	impl Default for TestState {
		fn default() -> Self {
			let para_id = ParaId::from(1);

			let validators = vec![
				Sr25519Keyring::Alice,
				Sr25519Keyring::Bob,
				Sr25519Keyring::Charlie,
				Sr25519Keyring::Dave,
				Sr25519Keyring::Ferdie,
			];

			let validator_public = validator_pubkeys(&validators);
			let discovery_keys = validator_authority_id(&validators);

			let validator_peer_id = std::iter::repeat_with(|| PeerId::random())
				.take(discovery_keys.len())
				.collect();

			let validator_groups = vec![vec![2, 0, 4], vec![3, 2, 4]]
				.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
			let group_rotation_info = GroupRotationInfo {
				session_start_block: 0,
				group_rotation_frequency: 100,
				now: 1,
			};

			let availability_core = CoreState::Scheduled(ScheduledCore {
				para_id,
				collator: None,
			});

			let relay_parent = Hash::random();

			let local_peer_id = PeerId::random();
			let collator_pair = CollatorPair::generate().0;

			Self {
				para_id,
				validators,
				session_info: SessionInfo {
					validators: validator_public,
					discovery_keys,
					validator_groups,
					..Default::default()
				},
				group_rotation_info,
				validator_peer_id,
				relay_parent,
				availability_core,
				local_peer_id,
				collator_pair,
				session_index: 1,
			}
		}
	}

	impl TestState {
		fn current_group_validator_indices(&self) -> &[ValidatorIndex] {
			&self.session_info.validator_groups[0]
		}

		fn current_session_index(&self) -> SessionIndex {
			self.session_index
		}

		fn current_group_validator_peer_ids(&self) -> Vec<PeerId> {
			self.current_group_validator_indices().iter().map(|i| self.validator_peer_id[i.0 as usize].clone()).collect()
		}

		fn current_group_validator_authority_ids(&self) -> Vec<AuthorityDiscoveryId> {
			self.current_group_validator_indices()
				.iter()
				.map(|i| self.session_info.discovery_keys[i.0 as usize].clone())
				.collect()
		}

		/// Generate a new relay parent and inform the subsystem about the new view.
		///
		/// If `merge_views == true` it means the subsystem will be informed that we are working on the old `relay_parent`
		/// and the new one.
		async fn advance_to_new_round(&mut self, virtual_overseer: &mut VirtualOverseer, merge_views: bool) {
			let old_relay_parent = self.relay_parent;

			while self.relay_parent == old_relay_parent {
				self.relay_parent.randomize();
			}

			let our_view = if merge_views {
				our_view![old_relay_parent, self.relay_parent]
			} else {
				our_view![self.relay_parent]
			};

			overseer_send(
				virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(NetworkBridgeEvent::OurViewChange(our_view)),
			).await;
		}
	}

	type VirtualOverseer = test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>;

	struct TestHarness {
		virtual_overseer: VirtualOverseer,
	}

	fn test_harness<T: Future<Output = VirtualOverseer>>(
		local_peer_id: PeerId,
		collator_pair: CollatorPair,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = run(context, local_peer_id, collator_pair, Metrics::default());

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::join(async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		}, subsystem)).1.unwrap();
	}

	const TIMEOUT: Duration = Duration::from_millis(100);

	async fn overseer_send(
		overseer: &mut VirtualOverseer,
		msg: CollatorProtocolMessage,
	) {
		tracing::trace!(?msg, "sending message");
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
	}

	async fn overseer_recv(
		overseer: &mut VirtualOverseer,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

		tracing::trace!(?msg, "received message");

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut VirtualOverseer,
		timeout: Duration,
	) -> Option<AllMessages> {
		tracing::trace!("waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	async fn overseer_signal(
		overseer: &mut VirtualOverseer,
		signal: OverseerSignal,
	) {
		overseer
			.send(FromOverseer::Signal(signal))
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
	}

	// Setup the system by sending the `CollateOn`, `ActiveLeaves` and `OurViewChange` messages.
	async fn setup_system(virtual_overseer: &mut VirtualOverseer, test_state: &TestState) {
		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::CollateOn(test_state.para_id),
		).await;

		overseer_signal(
			virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![ActivatedLeaf {
					hash: test_state.relay_parent,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}].into(),
				deactivated: [][..].into(),
			}),
		).await;

		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent]),
			),
		).await;
	}

	/// Result of [`distribute_collation`]
	struct DistributeCollation {
		candidate: CandidateReceipt,
		pov_block: PoV,
	}

	/// Create some PoV and distribute it.
	async fn distribute_collation(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
		// whether or not we expect a connection request or not.
		should_connect: bool,
	) -> DistributeCollation {
		// Now we want to distribute a PoVBlock
		let pov_block = PoV {
			block_data: BlockData(vec![42, 43, 44]),
		};

		let pov_hash = pov_block.hash();

		let candidate = TestCandidateBuilder {
			para_id: test_state.para_id,
			relay_parent: test_state.relay_parent,
			pov_hash,
			..Default::default()
		}.build();

		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::DistributeCollation(candidate.clone(), pov_block.clone(), None),
		).await;

		// obtain the availability cores.
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				tx.send(Ok(vec![test_state.availability_core.clone()])).unwrap();
			}
		);

		// We don't know precisely what is going to come as session info might be cached:
		loop {
			match overseer_recv(virtual_overseer).await {
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::SessionIndexForChild(tx),
				)) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					tx.send(Ok(test_state.current_session_index())).unwrap();
				}

				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::SessionInfo(index, tx),
				)) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					assert_eq!(index, test_state.current_session_index());

					tx.send(Ok(Some(test_state.session_info.clone()))).unwrap();
				}

				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::ValidatorGroups(tx)
				)) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					tx.send(Ok((
						test_state.session_info.validator_groups.clone(),
						test_state.group_rotation_info.clone(),
					))).unwrap();
					// This call is mandatory - we are done:
					break;
				}
				other =>
					panic!("Unexpected message received: {:?}", other),
			}
		}

		if should_connect {
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ConnectToValidators {
						..
					}
				) => {}
			);
		}

		DistributeCollation {
			candidate,
			pov_block,
		}
	}

	/// Connect a peer
	async fn connect_peer(
		virtual_overseer: &mut VirtualOverseer,
		peer: PeerId,
		authority_id: Option<AuthorityDiscoveryId>
	) {
		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					polkadot_node_network_protocol::ObservedRole::Authority,
					authority_id,
				),
			),
		).await;

		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer, view![]),
			),
		).await;
	}

	/// Disconnect a peer
	async fn disconnect_peer(virtual_overseer: &mut VirtualOverseer, peer: PeerId) {
		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(NetworkBridgeEvent::PeerDisconnected(peer)),
		).await;
	}

	/// Check that the next received message is a `Declare` message.
	async fn expect_declare_msg(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
		peer: &PeerId,
	) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendCollationMessage(
					to,
					protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
				)
			) => {
				assert_eq!(to[0], *peer);
				assert_matches!(
					wire_message,
					protocol_v1::CollatorProtocolMessage::Declare(
						collator_id,
						para_id,
						signature,
					) => {
						assert!(signature.verify(
							&*protocol_v1::declare_signature_payload(&test_state.local_peer_id),
							&collator_id),
						);
						assert_eq!(collator_id, test_state.collator_pair.public());
						assert_eq!(para_id, test_state.para_id);
					}
				);
			}
		);
	}

	/// Check that the next received message is a collation advertisement message.
	async fn expect_advertise_collation_msg(
		virtual_overseer: &mut VirtualOverseer,
		peer: &PeerId,
		expected_relay_parent: Hash,
	) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendCollationMessage(
					to,
					protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
				)
			) => {
				assert_eq!(to[0], *peer);
				assert_matches!(
					wire_message,
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						relay_parent,
					) => {
						assert_eq!(relay_parent, expected_relay_parent);
					}
				);
			}
		);
	}

	/// Send a message that the given peer's view changed.
	async fn send_peer_view_change(virtual_overseer: &mut VirtualOverseer, peer: &PeerId, hashes: Vec<Hash>) {
		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::new(hashes, 0)),
			),
		).await;
	}

	#[test]
	fn advertise_and_send_collation() {
		let mut test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			setup_system(&mut virtual_overseer, &test_state).await;

			let DistributeCollation { candidate, pov_block } =
				distribute_collation(&mut virtual_overseer, &test_state, true).await;

			for (val, peer) in test_state.current_group_validator_authority_ids()
				.into_iter()
				.zip(test_state.current_group_validator_peer_ids())
			{
				connect_peer(&mut virtual_overseer, peer.clone(), Some(val.clone())).await;
			}

			// We declare to the connected validators that we are a collator.
			// We need to catch all `Declare` messages to the validators we've
			// previosly connected to.
			for peer_id in test_state.current_group_validator_peer_ids() {
				expect_declare_msg(&mut virtual_overseer, &test_state, &peer_id).await;
			}

			let peer = test_state.current_group_validator_peer_ids()[0].clone();

			// Send info about peer's view.
			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;

			// The peer is interested in a leaf that we have a collation for;
			// advertise it.
			expect_advertise_collation_msg(&mut virtual_overseer, &peer, test_state.relay_parent).await;

			// Request a collation.
			let (tx, rx) = oneshot::channel();
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::CollationFetchingRequest(
					IncomingRequest::new(
						peer,
						CollationFetchingRequest {
							relay_parent: test_state.relay_parent,
							para_id: test_state.para_id,
						},
						tx,
					)
				)
			).await;

			assert_matches!(
				rx.await,
				Ok(full_response) => {
					let CollationFetchingResponse::Collation(receipt, pov): CollationFetchingResponse
						= CollationFetchingResponse::decode(
							&mut full_response.result
							.expect("We should have a proper answer").as_ref()
					)
					.expect("Decoding should work");
					assert_eq!(receipt, candidate);
					assert_eq!(pov, pov_block);
				}
			);

			let old_relay_parent = test_state.relay_parent;
			test_state.advance_to_new_round(&mut virtual_overseer, false).await;

			let peer = test_state.validator_peer_id[2].clone();

			// Re-request a collation.
			let (tx, rx) = oneshot::channel();
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::CollationFetchingRequest(
					IncomingRequest::new(
						peer,
						CollationFetchingRequest {
							relay_parent: old_relay_parent,
							para_id: test_state.para_id,
						},
						tx,
					)
				)
			).await;
			// Re-requesting collation should fail:
			assert_matches!(
				rx.await,
				Err(_) => {}
			);

			assert!(overseer_recv_with_timeout(&mut virtual_overseer, TIMEOUT).await.is_none());

			distribute_collation(&mut virtual_overseer, &test_state, true).await;

			// Send info about peer's view.
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(
						peer.clone(),
						view![test_state.relay_parent],
					)
				)
			).await;

			expect_advertise_collation_msg(&mut virtual_overseer, &peer, test_state.relay_parent).await;
			virtual_overseer
		});
	}

	#[test]
	fn collators_declare_to_connected_peers() {
		let test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.validator_peer_id[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id)).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			virtual_overseer
		})
	}

	#[test]
	fn collations_are_only_advertised_to_validators_with_correct_view() {
		let test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			let peer2 = test_state.current_group_validator_peer_ids()[1].clone();
			let validator_id2 = test_state.current_group_validator_authority_ids()[1].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id)).await;

			// Connect the second validator
			connect_peer(&mut virtual_overseer, peer2.clone(), Some(validator_id2)).await;

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer2).await;

			// And let it tell us that it is has the same view.
			send_peer_view_change(&mut virtual_overseer, &peer2, vec![test_state.relay_parent]).await;

			distribute_collation(&mut virtual_overseer, &test_state, true).await;

			expect_advertise_collation_msg(&mut virtual_overseer, &peer2, test_state.relay_parent).await;

			// The other validator announces that it changed its view.
			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;

			// After changing the view we should receive the advertisement
			expect_advertise_collation_msg(&mut virtual_overseer, &peer, test_state.relay_parent).await;
			virtual_overseer
		})
	}

	#[test]
	fn collate_on_two_different_relay_chain_blocks() {
		let mut test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			let peer2 = test_state.current_group_validator_peer_ids()[1].clone();
			let validator_id2 = test_state.current_group_validator_authority_ids()[1].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id)).await;

			// Connect the second validator
			connect_peer(&mut virtual_overseer, peer2.clone(), Some(validator_id2)).await;

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer2).await;

			distribute_collation(&mut virtual_overseer, &test_state, true).await;

			let old_relay_parent = test_state.relay_parent;

			// Advance to a new round, while informing the subsystem that the old and the new relay parent are active.
			test_state.advance_to_new_round(&mut virtual_overseer, true).await;

			distribute_collation(&mut virtual_overseer, &test_state, true).await;

			send_peer_view_change(&mut virtual_overseer, &peer, vec![old_relay_parent]).await;
			expect_advertise_collation_msg(&mut virtual_overseer, &peer, old_relay_parent).await;

			send_peer_view_change(&mut virtual_overseer, &peer2, vec![test_state.relay_parent]).await;

			expect_advertise_collation_msg(&mut virtual_overseer, &peer2, test_state.relay_parent).await;
			virtual_overseer
		})
	}

	#[test]
	fn validator_reconnect_does_not_advertise_a_second_time() {
		let test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id.clone())).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;

			distribute_collation(&mut virtual_overseer, &test_state, true).await;

			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;
			expect_advertise_collation_msg(&mut virtual_overseer, &peer, test_state.relay_parent).await;

			// Disconnect and reconnect directly
			disconnect_peer(&mut virtual_overseer, peer.clone()).await;
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id)).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;

			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;

			assert!(overseer_recv_with_timeout(&mut virtual_overseer, TIMEOUT).await.is_none());
			virtual_overseer
		})
	}

	#[test]
	fn collators_reject_declare_messages() {
		let test_state = TestState::default();
		let local_peer_id = test_state.local_peer_id.clone();
		let collator_pair = test_state.collator_pair.clone();
		let collator_pair2 = CollatorPair::generate().0;

		test_harness(local_peer_id, collator_pair, |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone(), Some(validator_id)).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							collator_pair2.public(),
							ParaId::from(5),
							collator_pair2.sign(b"garbage"),
						),
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					p,
					PeerSet::Collation,
				)) if p == peer
			);
			virtual_overseer
		})
	}
}
