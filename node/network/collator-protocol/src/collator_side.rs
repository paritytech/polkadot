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

use super::{LOG_TARGET,  Result};

use futures::{select, FutureExt, channel::oneshot};

use polkadot_primitives::v1::{
	CollatorId, CoreIndex, CoreState, Hash, Id as ParaId, CandidateReceipt, PoV, ValidatorId, CandidateHash,
};
use polkadot_subsystem::{
	jaeger, PerLeafSpan,
	FromOverseer, OverseerSignal, SubsystemContext,
	messages::{AllMessages, CollatorProtocolMessage, NetworkBridgeMessage, NetworkBridgeEvent},
};
use polkadot_node_network_protocol::{
	peer_set::PeerSet, v1 as protocol_v1, View, PeerId, RequestId, OurView,
};
use polkadot_node_subsystem_util::{
	validator_discovery,
	request_validators_ctx,
	request_validator_groups_ctx,
	request_availability_cores_ctx,
	metrics::{self, prometheus},
};
use polkadot_node_primitives::{SignedFullStatement, Statement};

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

	/// Provide a timer for handling `ConnectionRequest` which observes on drop.
	fn time_handle_connection_request(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_connection_request.start_timer())
	}

	/// Provide a timer for `process_msg` which observes on drop.
	fn time_process_msg(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_msg.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	advertisements_made: prometheus::Counter<prometheus::U64>,
	collations_sent: prometheus::Counter<prometheus::U64>,
	handle_connection_request: prometheus::Histogram,
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
			handle_connection_request: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_collator_protocol_collator_handle_connection_request",
						"Time spent within `collator_protocol_collator::handle_connection_request`",
					)
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
struct ValidatorGroup {
	/// All [`ValidatorId`]'s that are assigned to us in this group.
	validator_ids: HashSet<ValidatorId>,
	/// The mapping from [`PeerId`] to [`ValidatorId`]. This is filled over time as we learn the [`PeerId`]'s from the
	/// authority discovery. It is not ensured that this will contain *all* validators of this group.
	peer_ids: HashMap<PeerId, ValidatorId>,
	/// All [`ValidatorId`]'s of the current group to that we advertised our collation.
	advertised_to: HashSet<ValidatorId>,
}

impl ValidatorGroup {
	/// Returns `true` if we should advertise our collation to the given peer.
	fn should_advertise_to(&self, peer: &PeerId) -> bool {
		match self.peer_ids.get(peer) {
			Some(validator_id) => !self.advertised_to.contains(validator_id),
			None => false,
		}
	}

	/// Should be called after we advertised our collation to the given `peer` to keep track of it.
	fn advertised_to_peer(&mut self, peer: &PeerId) {
		if let Some(validator_id) = self.peer_ids.get(peer) {
			self.advertised_to.insert(validator_id.clone());
		}
	}

	/// Add a [`PeerId`] that belongs to the given [`ValidatorId`].
	///
	/// This returns `true` if the given validator belongs to this group and we could insert its [`PeerId`].
	fn add_peer_id_for_validator(&mut self, peer_id: &PeerId, validator_id: &ValidatorId) -> bool {
		if !self.validator_ids.contains(validator_id) {
			false
		} else {
			self.peer_ids.insert(peer_id.clone(), validator_id.clone());
			true
		}
	}
}

impl From<HashSet<ValidatorId>> for ValidatorGroup {
	fn from(validator_ids: HashSet<ValidatorId>) -> Self {
		Self {
			validator_ids,
			peer_ids: HashMap::new(),
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

#[derive(Default)]
struct State {
	/// Our id.
	our_id: CollatorId,

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

	/// List of peers where we declared ourself as a collator.
	declared_at: HashSet<PeerId>,

	/// The connection requests to validators per relay parent.
	connection_requests: validator_discovery::ConnectionRequests,

	/// Metrics.
	metrics: Metrics,
}

impl State {
	/// Returns `true` if the given `peer` is interested in the leaf that is represented by `relay_parent`.
	fn peer_interested_in_leaf(&self, peer: &PeerId, relay_parent: &Hash) -> bool {
		self.peer_views.get(peer).map(|v| v.contains(relay_parent)).unwrap_or(false)
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
#[tracing::instrument(level = "trace", skip(ctx, state, pov), fields(subsystem = LOG_TARGET))]
async fn distribute_collation(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
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
			relay_parent = %relay_parent,
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
				relay_parent = %relay_parent,
				"looks like no core is assigned to {} at {}", id, relay_parent,
			);

			return Ok(())
		}
	};

	// Determine the group on that core and the next group on that core.
	let (current_validators, next_validators) = determine_our_validators(ctx, our_core, num_cores, relay_parent).await?;

	if current_validators.is_empty() && next_validators.is_empty() {
		tracing::warn!(
			target: LOG_TARGET,
			core = ?our_core,
			"there are no validators assigned to core",
		);

		return Ok(());
	}

	// Issue a discovery request for the validators of the current group and the next group.
	connect_to_validators(
		ctx,
		relay_parent,
		id,
		state,
		current_validators.union(&next_validators).cloned().collect(),
	).await?;

	state.our_validators_groups.insert(relay_parent, current_validators.into());

	if let Some(result_sender) = result_sender {
		state.collation_result_senders.insert(receipt.hash(), result_sender);
	}

	state.collations.insert(relay_parent, Collation { receipt, pov, status: CollationStatus::Created });

	Ok(())
}

/// Get the Id of the Core that is assigned to the para being collated on if any
/// and the total number of cores.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn determine_core(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	para_id: ParaId,
	relay_parent: Hash,
) -> Result<Option<(CoreIndex, usize)>> {
	let cores = request_availability_cores_ctx(relay_parent, ctx).await?.await??;

	for (idx, core) in cores.iter().enumerate() {
		if let CoreState::Scheduled(occupied) = core {
			if occupied.para_id == para_id {
				return Ok(Some(((idx as u32).into(), cores.len())));
			}
		}
	}

	Ok(None)
}

/// Figure out current and next group of validators assigned to the para being collated on.
///
/// Returns [`ValidatorId`]'s of current and next group as determined based on the `relay_parent`.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn determine_our_validators(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	core_index: CoreIndex,
	cores: usize,
	relay_parent: Hash,
) -> Result<(HashSet<ValidatorId>, HashSet<ValidatorId>)> {
	let groups = request_validator_groups_ctx(relay_parent, ctx).await?;

	let groups = groups.await??;

	let current_group_index = groups.1.group_for_core(core_index, cores);
	let current_validators = groups.0.get(current_group_index.0 as usize).map(|v| v.as_slice()).unwrap_or_default();

	let next_group_idx = (current_group_index.0 as usize + 1) % groups.0.len();
	let next_validators = groups.0.get(next_group_idx).map(|v| v.as_slice()).unwrap_or_default();

	let validators = request_validators_ctx(relay_parent, ctx).await?.await??;

	let current_validators = current_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();
	let next_validators = next_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();

	Ok((current_validators, next_validators))
}

/// Issue a `Declare` collation message to the given `peer`.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn declare(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	peer: PeerId,
) {
	let wire_message = protocol_v1::CollatorProtocolMessage::Declare(state.our_id.clone());

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendCollationMessage(
			vec![peer],
			protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
		)
	)).await;
}

/// Issue a connection request to a set of validators and
/// revoke the previous connection request.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn connect_to_validators(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	relay_parent: Hash,
	para_id: ParaId,
	state: &mut State,
	validators: Vec<ValidatorId>,
) -> Result<()> {
	let request = validator_discovery::connect_to_validators(
		ctx,
		relay_parent,
		validators,
		PeerSet::Collation,
	).await?;

	state.connection_requests.put(relay_parent, para_id, request);

	Ok(())
}

/// Advertise collation to the given `peer`.
///
/// This will only advertise a collation if there exists one for the given `relay_parent` and the given `peer` is
/// set as validator for our para at the given `relay_parent`.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn advertise_collation(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	relay_parent: Hash,
	peer: PeerId,
) {
	let collating_on = match state.collating_on {
		Some(collating_on) => collating_on,
		None => return,
	};

	let should_advertise = state.our_validators_groups
		.get(&relay_parent)
		.map(|g| g.should_advertise_to(&peer))
		.unwrap_or(false);

	match (state.collations.get_mut(&relay_parent), should_advertise) {
		(None, _) => {
			tracing::trace!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				peer_id = %peer,
				"No collation to advertise.",
			);
			return
		},
		(_, false) => {
			tracing::trace!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				peer_id = %peer,
				"Not advertising collation as we already advertised it to this validator.",
			);
			return
		}
		(Some(collation), true) => collation.status.advance_to_advertised(),
	}

	let wire_message = protocol_v1::CollatorProtocolMessage::AdvertiseCollation(relay_parent, collating_on);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendCollationMessage(
			vec![peer.clone()],
			protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
		)
	)).await;

	if let Some(validators) = state.our_validators_groups.get_mut(&relay_parent) {
		validators.advertised_to_peer(&peer);
	}

	state.metrics.on_advertisment_made();
}

/// The main incoming message dispatching switch.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn process_msg(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
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
			let _span2 = jaeger::pov_span(&pov, "distributing-collation");
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
					distribute_collation(ctx, state, id, receipt, pov, result_sender).await?;
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
		NotifyCollationSeconded(_, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"NotifyCollationSeconded message is not expected on the collator side of the protocol",
			);
		}
		NetworkBridgeUpdateV1(event) => {
			if let Err(e) = handle_network_msg(
				ctx,
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
	}

	Ok(())
}

/// Issue a response to a previously requested collation.
#[tracing::instrument(level = "trace", skip(ctx, state, pov), fields(subsystem = LOG_TARGET))]
async fn send_collation(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	request_id: RequestId,
	origin: PeerId,
	receipt: CandidateReceipt,
	pov: PoV,
) {
	let pov = match protocol_v1::CompressedPoV::compress(&pov) {
		Ok(pov) => pov,
		Err(error) => {
			tracing::debug!(
				target: LOG_TARGET,
				error = ?error,
				"Failed to create `CompressedPov`",
			);
			return
		}
	};

	let wire_message = protocol_v1::CollatorProtocolMessage::Collation(request_id, receipt, pov);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendCollationMessage(
			vec![origin],
			protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
		)
	)).await;

	state.metrics.on_collation_sent();
}

/// A networking messages switch.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_incoming_peer_message(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
) -> Result<()> {
	use protocol_v1::CollatorProtocolMessage::*;

	match msg {
		Declare(_) => {
			tracing::warn!(
				target: LOG_TARGET,
				"Declare message is not expected on the collator side of the protocol",
			);
		}
		AdvertiseCollation(_, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"AdvertiseCollation message is not expected on the collator side of the protocol",
			);
		}
		RequestCollation(request_id, relay_parent, para_id) => {
			let _span = state.span_per_relay_parent.get(&relay_parent).map(|s| s.child("request-collation"));
			match state.collating_on {
				Some(our_para_id) => {
					if our_para_id == para_id {
						let (receipt, pov) = if let Some(collation) = state.collations.get_mut(&relay_parent) {
							collation.status.advance_to_requested();
							(collation.receipt.clone(), collation.pov.clone())
						} else {
							tracing::warn!(
								target: LOG_TARGET,
								relay_parent = %relay_parent,
								"received a `RequestCollation` for a relay parent we don't have collation stored.",
							);

							return Ok(());
						};

						let _span = _span.as_ref().map(|s| s.child("sending"));
						send_collation(ctx, state, request_id, origin, receipt, pov).await;
					} else {
						tracing::warn!(
							target: LOG_TARGET,
							for_para_id = %para_id,
							our_para_id = %our_para_id,
							"received a `RequestCollation` for unexpected para_id",
						);
					}
				}
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						for_para_id = %para_id,
						"received a `RequestCollation` while not collating on any para",
					);
				}
			}
		}
		Collation(_, _, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"Collation message is not expected on the collator side of the protocol",
			);
		}
		CollationSeconded(statement) => {
			if !matches!(statement.payload(), Statement::Seconded(_)) {
				tracing::warn!(
					target: LOG_TARGET,
					statement = ?statement,
					"Collation seconded message received with none-seconded statement.",
				);
			} else if let Some(sender) = state.collation_result_senders.remove(&statement.payload().candidate_hash()) {
				let _ = sender.send(statement);
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

/// A validator is connected.
///
/// `Declare` that we are a collator with a given `CollatorId`.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_validator_connected(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	peer_id: PeerId,
	validator_id: ValidatorId,
	relay_parent: Hash,
) {
	let not_declared = state.declared_at.insert(peer_id.clone());

	if not_declared {
		declare(ctx, state, peer_id.clone()).await;
	}

	// Store the PeerId and find out if we should advertise to this peer.
	//
	// If this peer does not belong to the para validators, we also don't need to try to advertise our collation.
	let advertise = if let Some(validators) = state.our_validators_groups.get_mut(&relay_parent) {
		validators.add_peer_id_for_validator(&peer_id, &validator_id)
	} else {
		false
	};

	if advertise && state.peer_interested_in_leaf(&peer_id, &relay_parent) {
		advertise_collation(ctx, state, relay_parent, peer_id).await;
	}
}

/// Bridge messages switch.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_network_msg(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	bridge_message: NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>,
) -> Result<()> {
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(_peer_id, _observed_role) => {
			// If it is possible that a disconnected validator would attempt a reconnect
			// it should be handled here.
		}
		PeerViewChange(peer_id, view) => {
			handle_peer_view_change(ctx, state, peer_id, view).await;
		}
		PeerDisconnected(peer_id) => {
			state.peer_views.remove(&peer_id);
			state.declared_at.remove(&peer_id);
		}
		OurViewChange(view) => {
			handle_our_view_change(state, view).await?;
		}
		PeerMessage(remote, msg) => {
			handle_incoming_peer_message(ctx, state, remote, msg).await?;
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
		state.connection_requests.remove_all(removed);
		state.span_per_relay_parent.remove(removed);
	}

	state.view = view;

	Ok(())
}

/// The collator protocol collator side main loop.
#[tracing::instrument(skip(ctx, metrics), fields(subsystem = LOG_TARGET))]
pub(crate) async fn run(
	mut ctx: impl SubsystemContext<Message = CollatorProtocolMessage>,
	our_id: CollatorId,
	metrics: Metrics,
) -> Result<()> {
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State {
		metrics,
		our_id,
		..Default::default()
	};

	loop {
		select! {
			res = state.connection_requests.next().fuse() => {
				let _timer = state.metrics.time_handle_connection_request();

				handle_validator_connected(
					&mut ctx,
					&mut state,
					res.peer_id,
					res.validator_id,
					res.relay_parent,
				).await;
			},
			msg = ctx.recv().fuse() => match msg? {
				Communication { msg } => {
					if let Err(e) = process_msg(&mut ctx, &mut state, msg).await {
						tracing::warn!(target: LOG_TARGET, err = ?e, "Failed to process message");
					}
				},
				Signal(ActiveLeaves(_update)) => {}
				Signal(BlockFinalized(..)) => {}
				Signal(Conclude) => return Ok(()),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::{time::Duration, sync::Arc};

	use assert_matches::assert_matches;
	use futures::{executor, future, Future, channel::mpsc};

	use sp_core::crypto::Pair;
	use sp_keyring::Sr25519Keyring;

	use polkadot_primitives::v1::{
		BlockData, CandidateDescriptor, CollatorPair, ScheduledCore,
		ValidatorIndex, GroupRotationInfo, AuthorityDiscoveryId,
		SessionIndex, SessionInfo,
	};
	use polkadot_subsystem::{ActiveLeavesUpdate, messages::{RuntimeApiMessage, RuntimeApiRequest}, jaeger};
	use polkadot_node_subsystem_util::TimeoutExt;
	use polkadot_subsystem_testhelpers as test_helpers;
	use polkadot_node_network_protocol::{view, our_view};

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
		validator_public: Vec<ValidatorId>,
		validator_authority_id: Vec<AuthorityDiscoveryId>,
		validator_peer_id: Vec<PeerId>,
		validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
		relay_parent: Hash,
		availability_core: CoreState,
		our_collator_pair: CollatorPair,
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
			let validator_authority_id = validator_authority_id(&validators);

			let validator_peer_id = std::iter::repeat_with(|| PeerId::random())
				.take(validator_public.len())
				.collect();

			let validator_groups = vec![vec![2, 0, 4], vec![3, 2, 4]]
				.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
			let group_rotation_info = GroupRotationInfo {
				session_start_block: 0,
				group_rotation_frequency: 100,
				now: 1,
			};
			let validator_groups = (validator_groups, group_rotation_info);

			let availability_core = CoreState::Scheduled(ScheduledCore {
				para_id,
				collator: None,
			});

			let relay_parent = Hash::random();

			let our_collator_pair = CollatorPair::generate().0;

			Self {
				para_id,
				validators,
				validator_public,
				validator_authority_id,
				validator_peer_id,
				validator_groups,
				relay_parent,
				availability_core,
				our_collator_pair,
				session_index: 1,
			}
		}
	}

	impl TestState {
		fn current_group_validator_indices(&self) -> &[ValidatorIndex] {
			&self.validator_groups.0[0]
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
				.map(|i| self.validator_authority_id[i.0 as usize].clone())
				.collect()
		}

		fn current_group_validator_ids(&self) -> Vec<ValidatorId> {
			self.current_group_validator_indices()
				.iter()
				.map(|i| self.validator_public[i.0 as usize].clone())
				.collect()
		}

		fn next_group_validator_indices(&self) -> &[ValidatorIndex] {
			&self.validator_groups.0[1]
		}

		fn next_group_validator_authority_ids(&self) -> Vec<AuthorityDiscoveryId> {
			self.next_group_validator_indices()
				.iter()
				.map(|i| self.validator_authority_id[i.0 as usize].clone())
				.collect()
		}

		/// Generate a new relay parent and inform the subsystem about the new view.
		///
		/// If `merge_views == true` it means the subsystem will be informed that we working on the old `relay_parent`
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

	fn test_harness<T: Future<Output = ()>>(
		collator_id: CollatorId,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				Some("polkadot_collator_protocol"),
				log::LevelFilter::Trace,
			)
			.filter(
				Some(LOG_TARGET),
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = run(context, collator_id, Metrics::default());

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(100);

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		msg: CollatorProtocolMessage,
	) {
		tracing::trace!(msg = ?msg, "sending message");
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

		tracing::trace!(msg = ?msg, "received message");

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		timeout: Duration,
	) -> Option<AllMessages> {
		tracing::trace!("waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	async fn overseer_signal(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
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
				activated: [(test_state.relay_parent, Arc::new(jaeger::Span::Disabled))][..].into(),
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
		/// Should be used to inform the subsystem about connected validators.
		connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		candidate: CandidateReceipt,
		pov_block: PoV,
	}

	/// Create some PoV and distribute it.
	async fn distribute_collation(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
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

		// Obtain the validator groups
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::ValidatorGroups(tx)
			)) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				tx.send(Ok(test_state.validator_groups.clone())).unwrap();
			}
		);

		// obtain the validators per relay parent
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		// obtain the validator_id to authority_id mapping
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				tx.send(Ok(test_state.current_session_index())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(index, tx),
			)) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(index, test_state.current_session_index());

				let validators = test_state.current_group_validator_ids();
				let current_discovery_keys = test_state.current_group_validator_authority_ids();
				let next_discovery_keys = test_state.next_group_validator_authority_ids();

				let discovery_keys = [&current_discovery_keys[..], &next_discovery_keys[..]].concat();

				tx.send(Ok(Some(SessionInfo {
					validators,
					discovery_keys,
					..Default::default()
				}))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ConnectToValidators {
					connected,
					..
				}
			) => {
				DistributeCollation {
					connected,
					candidate,
					pov_block,
				}
			}
		)
	}

	/// Connect a peer
	async fn connect_peer(virtual_overseer: &mut VirtualOverseer, peer: PeerId) {
		overseer_send(
			virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					polkadot_node_network_protocol::ObservedRole::Authority,
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
	async fn expect_declare_msg(virtual_overseer: &mut VirtualOverseer, test_state: &TestState, peer: &PeerId) {
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
					protocol_v1::CollatorProtocolMessage::Declare(collator_id) => {
						assert_eq!(collator_id, test_state.our_collator_pair.public());
					}
				);
			}
		);
	}

	/// Check that the next received message is a collation advertisment message.
	async fn expect_advertise_collation_msg(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
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
						collating_on,
					) => {
						assert_eq!(relay_parent, expected_relay_parent);
						assert_eq!(collating_on, test_state.para_id);
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

		test_harness(test_state.our_collator_pair.public(), |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			setup_system(&mut virtual_overseer, &test_state).await;

			let DistributeCollation { mut connected, candidate, pov_block } =
				distribute_collation(&mut virtual_overseer, &test_state).await;
			test_state.current_group_validator_authority_ids()
				.into_iter()
				.zip(test_state.current_group_validator_peer_ids())
				.for_each(|r| connected.try_send(r).unwrap());

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
			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer, test_state.relay_parent).await;

			let request_id = 42;

			// Request a collation.
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer.clone(),
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							request_id,
							test_state.relay_parent,
							test_state.para_id,
						)
					)
				)
			).await;

			// Wait for the reply.
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendCollationMessage(
						to,
						protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
					)
				) => {
					assert_eq!(to, vec![peer]);
					assert_matches!(
						wire_message,
						protocol_v1::CollatorProtocolMessage::Collation(req_id, receipt, pov) => {
							assert_eq!(req_id, request_id);
							assert_eq!(receipt, candidate);
							assert_eq!(pov.decompress().unwrap(), pov_block);
						}
					);
				}
			);

			let old_relay_parent = test_state.relay_parent;
			test_state.advance_to_new_round(&mut virtual_overseer, false).await;

			let peer = test_state.validator_peer_id[2].clone();

			// Re-request a collation.
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer.clone(),
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							43,
							old_relay_parent,
							test_state.para_id,
						)
					)
				)
			).await;

			assert!(overseer_recv_with_timeout(&mut virtual_overseer, TIMEOUT).await.is_none());

			let DistributeCollation { mut connected, .. } =
				distribute_collation(&mut virtual_overseer, &test_state).await;
			test_state.current_group_validator_authority_ids()
				.into_iter()
				.zip(test_state.current_group_validator_peer_ids())
				.for_each(|r| connected.try_send(r).unwrap());

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

			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer, test_state.relay_parent).await;
		});
	}

	/// This test ensures that we declare a collator at a validator by sending the `Declare` message as soon as the
	/// collator is aware of the validator being connected.
	#[test]
	fn collators_are_registered_correctly_at_validators() {
		let test_state = TestState::default();

		test_harness(test_state.our_collator_pair.public(), |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.validator_peer_id[0].clone();
			let validator_id = test_state.validator_authority_id[0].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone()).await;

			let mut connected = distribute_collation(&mut virtual_overseer, &test_state).await.connected;
			connected.try_send((validator_id, peer.clone())).unwrap();

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
		})
	}

	#[test]
	fn collations_are_only_advertised_to_validators_with_correct_view() {
		let test_state = TestState::default();

		test_harness(test_state.our_collator_pair.public(), |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			let peer2 = test_state.current_group_validator_peer_ids()[1].clone();
			let validator_id2 = test_state.current_group_validator_authority_ids()[1].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone()).await;

			// Connect the second validator
			connect_peer(&mut virtual_overseer, peer2.clone()).await;

			// And let it tell us that it is has the same view.
			send_peer_view_change(&mut virtual_overseer, &peer2, vec![test_state.relay_parent]).await;

			let mut connected = distribute_collation(&mut virtual_overseer, &test_state).await.connected;
			connected.try_send((validator_id, peer.clone())).unwrap();
			connected.try_send((validator_id2, peer2.clone())).unwrap();

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer2).await;

			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer2, test_state.relay_parent).await;

			// The other validator announces that it changed its view.
			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;

			// After changing the view we should receive the advertisement
			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer, test_state.relay_parent).await;
		})
	}

	#[test]
	fn collate_on_two_different_relay_chain_blocks() {
		let mut test_state = TestState::default();

		test_harness(test_state.our_collator_pair.public(), |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			let peer2 = test_state.current_group_validator_peer_ids()[1].clone();
			let validator_id2 = test_state.current_group_validator_authority_ids()[1].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone()).await;

			// Connect the second validator
			connect_peer(&mut virtual_overseer, peer2.clone()).await;

			let mut connected = distribute_collation(&mut virtual_overseer, &test_state).await.connected;
			connected.try_send((validator_id.clone(), peer.clone())).unwrap();
			connected.try_send((validator_id2.clone(), peer2.clone())).unwrap();

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			expect_declare_msg(&mut virtual_overseer, &test_state, &peer2).await;

			let old_relay_parent = test_state.relay_parent;

			// Advance to a new round, while informing the subsystem that the old and the new relay parent are active.
			test_state.advance_to_new_round(&mut virtual_overseer, true).await;

			let mut connected = distribute_collation(&mut virtual_overseer, &test_state).await.connected;
			connected.try_send((validator_id, peer.clone())).unwrap();
			connected.try_send((validator_id2, peer2.clone())).unwrap();

			send_peer_view_change(&mut virtual_overseer, &peer, vec![old_relay_parent]).await;
			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer, old_relay_parent).await;

			send_peer_view_change(&mut virtual_overseer, &peer2, vec![test_state.relay_parent]).await;
			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer2, test_state.relay_parent).await;
		})
	}

	#[test]
	fn validator_reconnect_does_not_advertise_a_second_time() {
		let test_state = TestState::default();

		test_harness(test_state.our_collator_pair.public(), |test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;

			let peer = test_state.current_group_validator_peer_ids()[0].clone();
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();

			setup_system(&mut virtual_overseer, &test_state).await;

			// A validator connected to us
			connect_peer(&mut virtual_overseer, peer.clone()).await;

			let mut connected = distribute_collation(&mut virtual_overseer, &test_state).await.connected;
			connected.try_send((validator_id.clone(), peer.clone())).unwrap();

			expect_declare_msg(&mut virtual_overseer, &test_state, &peer).await;
			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;
			expect_advertise_collation_msg(&mut virtual_overseer, &test_state, &peer, test_state.relay_parent).await;

			// Disconnect and reconnect directly
			disconnect_peer(&mut virtual_overseer, peer.clone()).await;
			connect_peer(&mut virtual_overseer, peer.clone()).await;
			send_peer_view_change(&mut virtual_overseer, &peer, vec![test_state.relay_parent]).await;

			assert!(overseer_recv_with_timeout(&mut virtual_overseer, TIMEOUT).await.is_none());
		})
	}
}
