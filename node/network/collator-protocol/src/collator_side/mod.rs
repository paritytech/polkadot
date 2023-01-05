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

use std::{
	collections::{HashMap, HashSet, VecDeque},
	pin::Pin,
	time::{Duration, Instant},
};

use futures::{
	channel::oneshot, pin_mut, select, stream::FuturesUnordered, Future, FutureExt, StreamExt,
};
use sp_core::Pair;

use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::PeerSet,
	request_response::{
		incoming::{self, OutgoingResponse},
		v1::{self as request_v1, CollationFetchingRequest, CollationFetchingResponse},
		IncomingRequest, IncomingRequestReceiver,
	},
	v1 as protocol_v1, OurView, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{CollationSecondedSignal, PoV, Statement};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		CollatorProtocolMessage, NetworkBridgeEvent, NetworkBridgeTxMessage, RuntimeApiMessage,
	},
	overseer, FromOrchestra, OverseerSignal, PerLeafSpan,
};
use polkadot_node_subsystem_util::{
	runtime::{get_availability_cores, get_group_rotation_info, RuntimeInfo},
	TimeoutExt,
};
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, CandidateHash, CandidateReceipt, CollatorPair, CoreIndex, CoreState,
	GroupIndex, Hash, Id as ParaId, SessionIndex,
};

use super::LOG_TARGET;
use crate::error::{log_error, Error, FatalError, Result};
use fatality::Split;

mod metrics;
mod validators_buffer;

use validators_buffer::{ValidatorGroupsBuffer, VALIDATORS_BUFFER_CAPACITY};

pub use metrics::Metrics;

#[cfg(test)]
mod tests;

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Peer sent unparsable request");
const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("An unexpected message");
const COST_APPARENT_FLOOD: Rep =
	Rep::CostMinor("Message received when previous one was still being processed");

/// Time after starting an upload to a validator we will start another one to the next validator,
/// even if the upload was not finished yet.
///
/// This is to protect from a single slow validator preventing collations from happening.
///
/// For considerations on this value, see: https://github.com/paritytech/polkadot/issues/4386
const MAX_UNSHARED_UPLOAD_TIME: Duration = Duration::from_millis(150);

/// Ensure that collator issues a connection request at least once every this many seconds.
/// Usually it's done when advertising new collation. However, if the core stays occupied or
/// it's not our turn to produce a candidate, it's important to disconnect from previous
/// peers.
///
/// Validators are obtained from [`ValidatorGroupsBuffer::validators_to_connect`].
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(12);

/// How often to check for reconnect timeout.
const RECONNECT_POLL: Duration = Duration::from_secs(1);

/// Info about validators we are currently connected to.
///
/// It keeps track to which validators we advertised our collation.
#[derive(Debug)]
struct ValidatorGroup {
	/// All [`ValidatorId`]'s of the current group to that we advertised our collation.
	advertised_to: HashSet<AuthorityDiscoveryId>,
}

impl ValidatorGroup {
	/// Create a new `ValidatorGroup`
	///
	/// without any advertisements.
	fn new() -> Self {
		Self { advertised_to: HashSet::new() }
	}

	/// Returns `true` if we should advertise our collation to the given peer.
	fn should_advertise_to(
		&self,
		peer_ids: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
		peer: &PeerId,
	) -> bool {
		match peer_ids.get(peer) {
			Some(discovery_ids) => !discovery_ids.iter().any(|d| self.advertised_to.contains(d)),
			None => false,
		}
	}

	/// Should be called after we advertised our collation to the given `peer` to keep track of it.
	fn advertised_to_peer(
		&mut self,
		peer_ids: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
		peer: &PeerId,
	) {
		if let Some(authority_ids) = peer_ids.get(peer) {
			authority_ids.iter().for_each(|a| {
				self.advertised_to.insert(a.clone());
			});
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

/// Stores the state for waiting collation fetches.
#[derive(Default)]
struct WaitingCollationFetches {
	/// Is there currently a collation getting fetched?
	collation_fetch_active: bool,
	/// The collation fetches waiting to be fulfilled.
	waiting: VecDeque<IncomingRequest<CollationFetchingRequest>>,
	/// All peers that are waiting or actively uploading.
	///
	/// We will not accept multiple requests from the same peer, otherwise our DoS protection of
	/// moving on to the next peer after `MAX_UNSHARED_UPLOAD_TIME` would be pointless.
	waiting_peers: HashSet<PeerId>,
}

struct CollationSendResult {
	relay_parent: Hash,
	peer_id: PeerId,
	timed_out: bool,
}

type ActiveCollationFetches =
	FuturesUnordered<Pin<Box<dyn Future<Output = CollationSendResult> + Send + 'static>>>;

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
	collation_result_senders: HashMap<CandidateHash, oneshot::Sender<CollationSecondedSignal>>,

	/// Our validator groups per active leaf.
	our_validators_groups: HashMap<Hash, ValidatorGroup>,

	/// The mapping from [`PeerId`] to [`HashSet<AuthorityDiscoveryId>`]. This is filled over time as we learn the [`PeerId`]'s
	/// by `PeerConnected` events.
	peer_ids: HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,

	/// Tracks which validators we want to stay connected to.
	validator_groups_buf: ValidatorGroupsBuffer,

	/// Timestamp of the last connection request to a non-empty list of validators,
	/// `None` otherwise.
	last_connected_at: Option<Instant>,

	/// Metrics.
	metrics: Metrics,

	/// All collation fetching requests that are still waiting to be answered.
	///
	/// They are stored per relay parent, when our view changes and the relay parent moves out, we will cancel the fetch
	/// request.
	waiting_collation_fetches: HashMap<Hash, WaitingCollationFetches>,

	/// Active collation fetches.
	///
	/// Each future returns the relay parent of the finished collation fetch.
	active_collation_fetches: ActiveCollationFetches,
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
			validator_groups_buf: ValidatorGroupsBuffer::with_capacity(VALIDATORS_BUFFER_CAPACITY),
			last_connected_at: None,
			waiting_collation_fetches: Default::default(),
			active_collation_fetches: Default::default(),
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
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn distribute_collation<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	id: ParaId,
	receipt: CandidateReceipt,
	pov: PoV,
	result_sender: Option<oneshot::Sender<CollationSecondedSignal>>,
) -> Result<()> {
	let relay_parent = receipt.descriptor.relay_parent;
	let candidate_hash = receipt.hash();

	// This collation is not in the active-leaves set.
	if !state.view.contains(&relay_parent) {
		gum::warn!(
			target: LOG_TARGET,
			?relay_parent,
			"distribute collation message parent is outside of our view",
		);

		return Ok(())
	}

	// We have already seen collation for this relay parent.
	if state.collations.contains_key(&relay_parent) {
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			"Already seen collation for this relay parent",
		);
		return Ok(())
	}

	// Determine which core the para collated-on is assigned to.
	// If it is not scheduled then ignore the message.
	let (our_core, num_cores) = match determine_core(ctx.sender(), id, relay_parent).await? {
		Some(core) => core,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				para_id = %id,
				?relay_parent,
				"looks like no core is assigned to {} at {}", id, relay_parent,
			);

			return Ok(())
		},
	};

	// Determine the group on that core.
	let GroupValidators { validators, session_index, group_index } =
		determine_our_validators(ctx, runtime, our_core, num_cores, relay_parent).await?;

	if validators.is_empty() {
		gum::warn!(
			target: LOG_TARGET,
			core = ?our_core,
			"there are no validators assigned to core",
		);

		return Ok(())
	}

	// It's important to insert new collation bits **before**
	// issuing a connection request.
	//
	// If a validator managed to fetch all the relevant collations
	// but still assigned to our core, we keep the connection alive.
	state.validator_groups_buf.note_collation_advertised(
		relay_parent,
		session_index,
		group_index,
		&validators,
	);

	gum::debug!(
		target: LOG_TARGET,
		para_id = %id,
		relay_parent = %relay_parent,
		?candidate_hash,
		pov_hash = ?pov.hash(),
		core = ?our_core,
		current_validators = ?validators,
		"Accepted collation, connecting to validators."
	);

	// Update a set of connected validators if necessary.
	state.last_connected_at = connect_to_validators(ctx, &state.validator_groups_buf).await;

	state.our_validators_groups.insert(relay_parent, ValidatorGroup::new());

	if let Some(result_sender) = result_sender {
		state.collation_result_senders.insert(candidate_hash, result_sender);
	}

	state
		.collations
		.insert(relay_parent, Collation { receipt, pov, status: CollationStatus::Created });

	let interested = state.peers_interested_in_leaf(&relay_parent);
	// Make sure already connected peers get collations:
	for peer_id in interested {
		advertise_collation(ctx, state, relay_parent, peer_id).await;
	}

	Ok(())
}

/// Get the Id of the Core that is assigned to the para being collated on if any
/// and the total number of cores.
async fn determine_core(
	sender: &mut impl overseer::SubsystemSender<RuntimeApiMessage>,
	para_id: ParaId,
	relay_parent: Hash,
) -> Result<Option<(CoreIndex, usize)>> {
	let cores = get_availability_cores(sender, relay_parent).await?;

	for (idx, core) in cores.iter().enumerate() {
		if let CoreState::Scheduled(occupied) = core {
			if occupied.para_id == para_id {
				return Ok(Some(((idx as u32).into(), cores.len())))
			}
		}
	}

	Ok(None)
}

/// Validators of a particular group index.
#[derive(Debug)]
struct GroupValidators {
	/// The validators of above group (their discovery keys).
	validators: Vec<AuthorityDiscoveryId>,

	session_index: SessionIndex,
	group_index: GroupIndex,
}

/// Figure out current group of validators assigned to the para being collated on.
///
/// Returns [`ValidatorId`]'s of current group as determined based on the `relay_parent`.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn determine_our_validators<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	core_index: CoreIndex,
	cores: usize,
	relay_parent: Hash,
) -> Result<GroupValidators> {
	let session_index = runtime.get_session_index_for_child(ctx.sender(), relay_parent).await?;
	let info = &runtime
		.get_session_info_by_index(ctx.sender(), relay_parent, session_index)
		.await?
		.session_info;
	gum::debug!(target: LOG_TARGET, ?session_index, "Received session info");
	let groups = &info.validator_groups;
	let rotation_info = get_group_rotation_info(ctx.sender(), relay_parent).await?;

	let current_group_index = rotation_info.group_for_core(core_index, cores);
	let current_validators =
		groups.get(current_group_index).map(|v| v.as_slice()).unwrap_or_default();

	let validators = &info.discovery_keys;

	let current_validators =
		current_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();

	let current_validators = GroupValidators {
		validators: current_validators,
		session_index,
		group_index: current_group_index,
	};

	Ok(current_validators)
}

/// Issue a `Declare` collation message to the given `peer`.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn declare<Context>(ctx: &mut Context, state: &mut State, peer: PeerId) {
	let declare_signature_payload = protocol_v1::declare_signature_payload(&state.local_peer_id);

	if let Some(para_id) = state.collating_on {
		let wire_message = protocol_v1::CollatorProtocolMessage::Declare(
			state.collator_pair.public(),
			para_id,
			state.collator_pair.sign(&declare_signature_payload),
		);

		ctx.send_message(NetworkBridgeTxMessage::SendCollationMessage(
			vec![peer],
			Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(wire_message)),
		))
		.await;
	}
}

/// Updates a set of connected validators based on their advertisement-bits
/// in a validators buffer.
///
/// Returns current timestamp if the connection request was non-empty, `None`
/// otherwise.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn connect_to_validators<Context>(
	ctx: &mut Context,
	validator_groups_buf: &ValidatorGroupsBuffer,
) -> Option<Instant> {
	let validator_ids = validator_groups_buf.validators_to_connect();
	let is_disconnect = validator_ids.is_empty();

	// ignore address resolution failure
	// will reissue a new request on new collation
	let (failed, _) = oneshot::channel();
	ctx.send_message(NetworkBridgeTxMessage::ConnectToValidators {
		validator_ids,
		peer_set: PeerSet::Collation,
		failed,
	})
	.await;

	(!is_disconnect).then_some(Instant::now())
}

/// Advertise collation to the given `peer`.
///
/// This will only advertise a collation if there exists one for the given `relay_parent` and the given `peer` is
/// set as validator for our para at the given `relay_parent`.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn advertise_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	peer: PeerId,
) {
	let should_advertise = state
		.our_validators_groups
		.get(&relay_parent)
		.map(|g| g.should_advertise_to(&state.peer_ids, &peer))
		.unwrap_or(false);

	match (state.collations.get_mut(&relay_parent), should_advertise) {
		(None, _) => {
			gum::trace!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"No collation to advertise.",
			);
			return
		},
		(_, false) => {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"Not advertising collation as we already advertised it to this validator.",
			);
			return
		},
		(Some(collation), true) => {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"Advertising collation.",
			);
			collation.status.advance_to_advertised()
		},
	}

	let wire_message = protocol_v1::CollatorProtocolMessage::AdvertiseCollation(relay_parent);

	ctx.send_message(NetworkBridgeTxMessage::SendCollationMessage(
		vec![peer],
		Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(wire_message)),
	))
	.await;

	if let Some(validators) = state.our_validators_groups.get_mut(&relay_parent) {
		validators.advertised_to_peer(&state.peer_ids, &peer);
	}

	state.metrics.on_advertisment_made();
}

/// The main incoming message dispatching switch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn process_msg<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	msg: CollatorProtocolMessage,
) -> Result<()> {
	use CollatorProtocolMessage::*;

	match msg {
		CollateOn(id) => {
			state.collating_on = Some(id);
		},
		DistributeCollation(receipt, pov, result_sender) => {
			let _span1 = state
				.span_per_relay_parent
				.get(&receipt.descriptor.relay_parent)
				.map(|s| s.child("distributing-collation"));
			let _span2 = jaeger::Span::new(&pov, "distributing-collation");
			match state.collating_on {
				Some(id) if receipt.descriptor.para_id != id => {
					// If the ParaId of a collation requested to be distributed does not match
					// the one we expect, we ignore the message.
					gum::warn!(
						target: LOG_TARGET,
						para_id = %receipt.descriptor.para_id,
						collating_on = %id,
						"DistributeCollation for unexpected para_id",
					);
				},
				Some(id) => {
					let _ = state.metrics.time_collation_distribution("distribute");
					distribute_collation(ctx, runtime, state, id, receipt, pov, result_sender)
						.await?;
				},
				None => {
					gum::warn!(
						target: LOG_TARGET,
						para_id = %receipt.descriptor.para_id,
						"DistributeCollation message while not collating on any",
					);
				},
			}
		},
		ReportCollator(_) => {
			gum::warn!(
				target: LOG_TARGET,
				"ReportCollator message is not expected on the collator side of the protocol",
			);
		},
		NetworkBridgeUpdate(event) => {
			// We should count only this shoulder in the histogram, as other shoulders are just introducing noise
			let _ = state.metrics.time_process_msg();

			if let Err(e) = handle_network_msg(ctx, runtime, state, event).await {
				gum::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to handle incoming network message",
				);
			}
		},
		_ => {},
	}

	Ok(())
}

/// Issue a response to a previously requested collation.
async fn send_collation(
	state: &mut State,
	request: IncomingRequest<CollationFetchingRequest>,
	receipt: CandidateReceipt,
	pov: PoV,
) {
	let (tx, rx) = oneshot::channel();

	let relay_parent = request.payload.relay_parent;
	let peer_id = request.peer;

	let response = OutgoingResponse {
		result: Ok(CollationFetchingResponse::Collation(receipt, pov)),
		reputation_changes: Vec::new(),
		sent_feedback: Some(tx),
	};

	if let Err(_) = request.send_outgoing_response(response) {
		gum::warn!(target: LOG_TARGET, "Sending collation response failed");
	}

	state.active_collation_fetches.push(
		async move {
			let r = rx.timeout(MAX_UNSHARED_UPLOAD_TIME).await;
			let timed_out = r.is_none();

			CollationSendResult { relay_parent, peer_id, timed_out }
		}
		.boxed(),
	);

	state.metrics.on_collation_sent();
}

/// A networking messages switch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_incoming_peer_message<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
) -> Result<()> {
	use protocol_v1::CollatorProtocolMessage::*;

	match msg {
		Declare(_, _, _) => {
			gum::trace!(
				target: LOG_TARGET,
				?origin,
				"Declare message is not expected on the collator side of the protocol",
			);

			// If we are declared to, this is another collator, and we should disconnect.
			ctx.send_message(NetworkBridgeTxMessage::DisconnectPeer(origin, PeerSet::Collation))
				.await;
		},
		AdvertiseCollation(_) => {
			gum::trace!(
				target: LOG_TARGET,
				?origin,
				"AdvertiseCollation message is not expected on the collator side of the protocol",
			);

			ctx.send_message(NetworkBridgeTxMessage::ReportPeer(origin, COST_UNEXPECTED_MESSAGE))
				.await;

			// If we are advertised to, this is another collator, and we should disconnect.
			ctx.send_message(NetworkBridgeTxMessage::DisconnectPeer(origin, PeerSet::Collation))
				.await;
		},
		CollationSeconded(relay_parent, statement) => {
			if !matches!(statement.unchecked_payload(), Statement::Seconded(_)) {
				gum::warn!(
					target: LOG_TARGET,
					?statement,
					?origin,
					"Collation seconded message received with none-seconded statement.",
				);
			} else {
				let statement = runtime
					.check_signature(ctx.sender(), relay_parent, statement)
					.await?
					.map_err(Error::InvalidStatementSignature)?;

				let removed =
					state.collation_result_senders.remove(&statement.payload().candidate_hash());

				if let Some(sender) = removed {
					gum::trace!(
						target: LOG_TARGET,
						?statement,
						?origin,
						"received a valid `CollationSeconded`",
					);
					let _ = sender.send(CollationSecondedSignal { statement, relay_parent });
				} else {
					gum::debug!(
						target: LOG_TARGET,
						candidate_hash = ?&statement.payload().candidate_hash(),
						?origin,
						"received an unexpected `CollationSeconded`: unknown statement",
					);
				}
			}
		},
	}

	Ok(())
}

/// Process an incoming network request for a collation.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_incoming_request<Context>(
	ctx: &mut Context,
	state: &mut State,
	req: IncomingRequest<request_v1::CollationFetchingRequest>,
) -> Result<()> {
	let _span = state
		.span_per_relay_parent
		.get(&req.payload.relay_parent)
		.map(|s| s.child("request-collation"));

	match state.collating_on {
		Some(our_para_id) if our_para_id == req.payload.para_id => {
			let (receipt, pov) =
				if let Some(collation) = state.collations.get_mut(&req.payload.relay_parent) {
					collation.status.advance_to_requested();
					(collation.receipt.clone(), collation.pov.clone())
				} else {
					gum::warn!(
						target: LOG_TARGET,
						relay_parent = %req.payload.relay_parent,
						"received a `RequestCollation` for a relay parent we don't have collation stored.",
					);

					return Ok(())
				};

			state.metrics.on_collation_sent_requested();

			let _span = _span.as_ref().map(|s| s.child("sending"));

			let waiting =
				state.waiting_collation_fetches.entry(req.payload.relay_parent).or_default();

			if !waiting.waiting_peers.insert(req.peer) {
				gum::debug!(
					target: LOG_TARGET,
					"Dropping incoming request as peer has a request in flight already."
				);
				ctx.send_message(NetworkBridgeTxMessage::ReportPeer(req.peer, COST_APPARENT_FLOOD))
					.await;
				return Ok(())
			}

			if waiting.collation_fetch_active {
				waiting.waiting.push_back(req);
			} else {
				waiting.collation_fetch_active = true;
				// Obtain a timer for sending collation
				let _ = state.metrics.time_collation_distribution("send");
				send_collation(state, req, receipt, pov).await;
			}
		},
		Some(our_para_id) => {
			gum::warn!(
				target: LOG_TARGET,
				for_para_id = %req.payload.para_id,
				our_para_id = %our_para_id,
				"received a `CollationFetchingRequest` for unexpected para_id",
			);
		},
		None => {
			gum::warn!(
				target: LOG_TARGET,
				for_para_id = %req.payload.para_id,
				"received a `RequestCollation` while not collating on any para",
			);
		},
	}
	Ok(())
}

/// Our view has changed.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_peer_view_change<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer_id: PeerId,
	view: View,
) {
	let current = state.peer_views.entry(peer_id).or_default();

	let added: Vec<Hash> = view.difference(&*current).cloned().collect();

	*current = view;

	for added in added.into_iter() {
		advertise_collation(ctx, state, added, peer_id).await;
	}
}

/// Bridge messages switch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	state: &mut State,
	bridge_message: NetworkBridgeEvent<net_protocol::CollatorProtocolMessage>,
) -> Result<()> {
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(peer_id, observed_role, _, maybe_authority) => {
			// If it is possible that a disconnected validator would attempt a reconnect
			// it should be handled here.
			gum::trace!(target: LOG_TARGET, ?peer_id, ?observed_role, "Peer connected");
			if let Some(authority_ids) = maybe_authority {
				gum::trace!(
					target: LOG_TARGET,
					?authority_ids,
					?peer_id,
					"Connected to requested validator"
				);
				state.peer_ids.insert(peer_id, authority_ids);

				declare(ctx, state, peer_id).await;
			}
		},
		PeerViewChange(peer_id, view) => {
			gum::trace!(target: LOG_TARGET, ?peer_id, ?view, "Peer view change");
			handle_peer_view_change(ctx, state, peer_id, view).await;
		},
		PeerDisconnected(peer_id) => {
			gum::trace!(target: LOG_TARGET, ?peer_id, "Peer disconnected");
			state.peer_views.remove(&peer_id);
			state.peer_ids.remove(&peer_id);
		},
		OurViewChange(view) => {
			gum::trace!(target: LOG_TARGET, ?view, "Own view change");
			handle_our_view_change(state, view).await?;
		},
		PeerMessage(remote, Versioned::V1(msg)) => {
			handle_incoming_peer_message(ctx, runtime, state, remote, msg).await?;
		},
		NewGossipTopology { .. } => {
			// impossible!
		},
	}

	Ok(())
}

/// Handles our view changes.
async fn handle_our_view_change(state: &mut State, view: OurView) -> Result<()> {
	for removed in state.view.difference(&view) {
		gum::debug!(target: LOG_TARGET, relay_parent = ?removed, "Removing relay parent because our view changed.");

		if let Some(collation) = state.collations.remove(removed) {
			state.collation_result_senders.remove(&collation.receipt.hash());

			match collation.status {
				CollationStatus::Created => gum::warn!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation wasn't advertised to any validator.",
				),
				CollationStatus::Advertised => gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation was advertised but not requested by any validator.",
				),
				CollationStatus::Requested => gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?collation.receipt.hash(),
					pov_hash = ?collation.pov.hash(),
					"Collation was requested.",
				),
			}
		}
		state.our_validators_groups.remove(removed);
		state.span_per_relay_parent.remove(removed);
		state.waiting_collation_fetches.remove(removed);
		state.validator_groups_buf.remove_relay_parent(removed);
	}

	state.view = view;

	Ok(())
}

/// The collator protocol collator side main loop.
#[overseer::contextbounds(CollatorProtocol, prefix = crate::overseer)]
pub(crate) async fn run<Context>(
	mut ctx: Context,
	local_peer_id: PeerId,
	collator_pair: CollatorPair,
	mut req_receiver: IncomingRequestReceiver<request_v1::CollationFetchingRequest>,
	metrics: Metrics,
) -> std::result::Result<(), FatalError> {
	use OverseerSignal::*;

	let mut state = State::new(local_peer_id, collator_pair, metrics);
	let mut runtime = RuntimeInfo::new(None);

	let reconnect_stream = super::tick_stream(RECONNECT_POLL);
	pin_mut!(reconnect_stream);

	loop {
		let recv_req = req_receiver.recv(|| vec![COST_INVALID_REQUEST]).fuse();
		pin_mut!(recv_req);
		select! {
			msg = ctx.recv().fuse() => match msg.map_err(FatalError::SubsystemReceive)? {
				FromOrchestra::Communication { msg } => {
					log_error(
						process_msg(&mut ctx, &mut runtime, &mut state, msg).await,
						"Failed to process message"
					)?;
				},
				FromOrchestra::Signal(ActiveLeaves(_update)) => {}
				FromOrchestra::Signal(BlockFinalized(..)) => {}
				FromOrchestra::Signal(Conclude) => return Ok(()),
			},
			CollationSendResult {
				relay_parent,
				peer_id,
				timed_out,
			} = state.active_collation_fetches.select_next_some() => {
				if timed_out {
					gum::debug!(
						target: LOG_TARGET,
						?relay_parent,
						?peer_id,
						"Sending collation to validator timed out, carrying on with next validator",
					);
				} else {
					for authority_id in state.peer_ids.get(&peer_id).into_iter().flatten() {
						// Timeout not hit, this peer is no longer interested in this relay parent.
						state.validator_groups_buf.reset_validator_interest(relay_parent, authority_id);
					}
				}

				let next = if let Some(waiting) = state.waiting_collation_fetches.get_mut(&relay_parent) {
					waiting.waiting_peers.remove(&peer_id);
					if let Some(next) = waiting.waiting.pop_front() {
						next
					} else {
						waiting.collation_fetch_active = false;
						continue
					}
				} else {
					// No waiting collation fetches means we already removed the relay parent from our view.
					continue
				};

				if let Some(collation) = state.collations.get(&relay_parent) {
					let receipt = collation.receipt.clone();
					let pov = collation.pov.clone();

					send_collation(&mut state, next, receipt, pov).await;
				}
			},
			_ = reconnect_stream.next() => {
				let now = Instant::now();
				if state
					.last_connected_at
					.map_or(false, |timestamp| now - timestamp > RECONNECT_TIMEOUT)
				{
					// Remove all advertisements from the buffer if the timeout was hit.
					// Usually, it shouldn't be necessary as leaves get deactivated, rather
					// serves as a safeguard against finality lags.
					state.validator_groups_buf.clear_advertisements();
					// Returns `None` if connection request is empty.
					state.last_connected_at =
						connect_to_validators(&mut ctx, &state.validator_groups_buf).await;

					gum::debug!(
						target: LOG_TARGET,
						timeout = ?RECONNECT_TIMEOUT,
						"Timeout hit, sent a connection request. Disconnected from all validators = {}",
						state.last_connected_at.is_none(),
					);
				}
			},
			in_req = recv_req => {
				match in_req {
					Ok(req) => {
						log_error(
							handle_incoming_request(&mut ctx, &mut state, req).await,
							"Handling incoming request"
						)?;
					}
					Err(error) => {
						let jfyi = error.split().map_err(incoming::Error::from)?;
						gum::debug!(
							target: LOG_TARGET,
							error = ?jfyi,
							"Decoding incoming request failed"
						);
						continue
					}
				}
			}
		}
	}
}
