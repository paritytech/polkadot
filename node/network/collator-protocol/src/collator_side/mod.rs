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
	collections::{HashMap, HashSet},
	time::Duration,
};

use bitvec::prelude::*;
use futures::{channel::oneshot, pin_mut, select, FutureExt, StreamExt};
use sp_core::Pair;

use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::PeerSet,
	request_response::{
		incoming::{self, OutgoingResponse},
		v1 as request_v1, vstaging as request_vstaging, IncomingRequestReceiver,
	},
	v1 as protocol_v1, vstaging as protocol_vstaging, OurView, PeerId,
	UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{CollationSecondedSignal, PoV, Statement};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		CollatorProtocolMessage, NetworkBridgeEvent, NetworkBridgeMessage, RuntimeApiMessage,
	},
	overseer, CollatorProtocolSenderTrait, FromOrchestra, OverseerSignal, PerLeafSpan,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::View as ImplicitView,
	runtime::{get_availability_cores, get_group_rotation_info, RuntimeInfo},
	TimeoutExt,
};
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, CandidateHash, CandidateReceipt, CollatorPair, CoreIndex, CoreState,
	Hash, Id as ParaId,
};

use super::{
	prospective_parachains_mode, ProspectiveParachainsMode, LOG_TARGET, MAX_CANDIDATE_DEPTH,
};
use crate::error::{log_error, Error, FatalError, Result};

mod collation;
mod metrics;
#[cfg(test)]
mod tests;

use collation::{
	ActiveCollationFetches, Collation, CollationStatus, VersionedCollationRequest,
	WaitingCollationFetches,
};

pub use metrics::Metrics;

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

/// Info about validators we are currently connected to.
///
/// It keeps track to which validators we advertised our collation.
#[derive(Debug, Default)]
struct ValidatorGroup {
	/// Validators discovery ids. Lazily initialized when first
	/// distributing a collation.
	validators: Vec<AuthorityDiscoveryId>,

	/// Bits indicating which validators have already seen the announcement
	/// per candidate.
	advertised_to: HashMap<CandidateHash, BitVec<u8, Lsb0>>,
}

impl ValidatorGroup {
	/// Returns `true` if we should advertise our collation to the given peer.
	fn should_advertise_to(
		&self,
		candidate_hash: &CandidateHash,
		peer_ids: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
		peer: &PeerId,
	) -> bool {
		let authority_ids = match peer_ids.get(peer) {
			Some(authority_ids) => authority_ids,
			None => return false,
		};

		for id in authority_ids {
			// One peer id may correspond to different discovery ids across sessions,
			// having a non-empty intersection is sufficient to assume that this peer
			// belongs to this particular validator group.
			let validator_index = match self.validators.iter().position(|v| v == id) {
				Some(idx) => idx,
				None => continue,
			};

			// Either the candidate is unseen by this validator group
			// or the corresponding bit is not set.
			if self
				.advertised_to
				.get(candidate_hash)
				.map_or(true, |advertised| !advertised[validator_index])
			{
				return true
			}
		}

		false
	}

	/// Should be called after we advertised our collation to the given `peer` to keep track of it.
	fn advertised_to_peer(
		&mut self,
		candidate_hash: &CandidateHash,
		peer_ids: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
		peer: &PeerId,
	) {
		if let Some(authority_ids) = peer_ids.get(peer) {
			for id in authority_ids {
				let validator_index = match self.validators.iter().position(|v| v == id) {
					Some(idx) => idx,
					None => continue,
				};
				self.advertised_to
					.entry(*candidate_hash)
					.or_insert_with(|| bitvec![u8, Lsb0; 0; self.validators.len()])
					.set(validator_index, true);
			}
		}
	}
}

struct PerRelayParent {
	prospective_parachains_mode: ProspectiveParachainsMode,
	/// Validators group responsible for backing candidates built
	/// on top of this relay parent.
	validator_group: ValidatorGroup,
	/// Distributed collations.
	collations: HashMap<CandidateHash, Collation>,
}

impl PerRelayParent {
	fn new(mode: ProspectiveParachainsMode) -> Self {
		Self {
			prospective_parachains_mode: mode,
			validator_group: ValidatorGroup::default(),
			collations: HashMap::new(),
		}
	}
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

	/// Leaves that do support asynchronous backing along with
	/// implicit ancestry. Leaves from the implicit view are present in
	/// `active_leaves`, the opposite doesn't hold true.
	///
	/// Relay-chain blocks which don't support prospective parachains are
	/// never included in the fragment trees of active leaves which do. In
	/// particular, this means that if a given relay parent belongs to implicit
	/// ancestry of some active leaf, then it does support prospective parachains.
	implicit_view: ImplicitView,

	/// All active leaves observed by us, including both that do and do not
	/// support prospective parachains. This mapping works as a replacement for
	/// [`polkadot_node_network_protocol::View`] and can be dropped once the transition
	/// to asynchronous backing is done.
	active_leaves: HashMap<Hash, ProspectiveParachainsMode>,

	/// Validators and distributed collations tracked for each relay parent from
	/// our view, including both leaves and implicit ancestry.
	per_relay_parent: HashMap<Hash, PerRelayParent>,

	/// Span per relay parent.
	span_per_relay_parent: HashMap<Hash, PerLeafSpan>,

	/// The result senders per collation.
	collation_result_senders: HashMap<CandidateHash, oneshot::Sender<CollationSecondedSignal>>,

	/// The mapping from [`PeerId`] to [`HashSet<AuthorityDiscoveryId>`]. This is filled over time as we learn the [`PeerId`]'s
	/// by `PeerConnected` events.
	peer_ids: HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,

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
			implicit_view: Default::default(),
			active_leaves: Default::default(),
			per_relay_parent: Default::default(),
			span_per_relay_parent: Default::default(),
			collation_result_senders: Default::default(),
			peer_ids: Default::default(),
			waiting_collation_fetches: Default::default(),
			active_collation_fetches: Default::default(),
		}
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
	parent_head_data_hash: Hash,
	pov: PoV,
	result_sender: Option<oneshot::Sender<CollationSecondedSignal>>,
) -> Result<()> {
	let candidate_relay_parent = receipt.descriptor.relay_parent;
	let candidate_hash = receipt.hash();

	let per_relay_parent = match state.per_relay_parent.get_mut(&candidate_relay_parent) {
		Some(per_relay_parent) => per_relay_parent,
		None => {
			gum::debug!(
				target: LOG_TARGET,
				para_id = %id,
				candidate_relay_parent = %candidate_relay_parent,
				candidate_hash = ?candidate_hash,
				"Candidate relay parent is out of our view",
			);
			return Ok(())
		},
	};
	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;

	let collations_limit = match relay_parent_mode {
		ProspectiveParachainsMode::Disabled => 1,
		ProspectiveParachainsMode::Enabled => MAX_CANDIDATE_DEPTH + 1,
	};

	if per_relay_parent.collations.len() >= collations_limit {
		gum::debug!(
			target: LOG_TARGET,
			?candidate_relay_parent,
			"The limit of {} collations per relay parent is already reached",
			collations_limit,
		);
		return Ok(())
	}

	// We have already seen collation for this relay parent.
	if per_relay_parent.collations.contains_key(&candidate_hash) {
		gum::debug!(
			target: LOG_TARGET,
			?candidate_relay_parent,
			"Already seen collation for this relay parent",
		);
		return Ok(())
	}

	// Determine which core the para collated-on is assigned to.
	// If it is not scheduled then ignore the message.
	let (our_core, num_cores) =
		match determine_core(ctx.sender(), id, candidate_relay_parent, relay_parent_mode).await? {
			Some(core) => core,
			None => {
				gum::warn!(
					target: LOG_TARGET,
					para_id = %id,
					"looks like no core is assigned to {} at {}", id, candidate_relay_parent,
				);

				return Ok(())
			},
		};

	// Determine the group on that core.
	//
	// When prospective parachains are disabled, candidate relay parent here is
	// guaranteed to be an active leaf.
	let current_validators =
		determine_our_validators(ctx, runtime, our_core, num_cores, candidate_relay_parent).await?;

	if current_validators.validators.is_empty() {
		gum::warn!(
			target: LOG_TARGET,
			core = ?our_core,
			"there are no validators assigned to core",
		);

		return Ok(())
	}

	gum::debug!(
		target: LOG_TARGET,
		para_id = %id,
		candidate_relay_parent = %candidate_relay_parent,
		relay_parent_mode = ?relay_parent_mode,
		candidate_hash = ?candidate_hash,
		pov_hash = ?pov.hash(),
		core = ?our_core,
		?current_validators,
		"Accepted collation, connecting to validators."
	);

	let validators_at_relay_parent = &mut per_relay_parent.validator_group.validators;
	if validators_at_relay_parent.is_empty() {
		*validators_at_relay_parent = current_validators.validators.clone();
	}

	// Issue a discovery request for the validators of the current group:
	//
	// TODO [now]: some kind of connection management is necessary to avoid
	// dropping peers from e.g. implicit view assignments.
	connect_to_validators(ctx, current_validators.validators.into_iter().collect()).await;

	if let Some(result_sender) = result_sender {
		state.collation_result_senders.insert(candidate_hash, result_sender);
	}

	per_relay_parent.collations.insert(
		candidate_hash,
		Collation { receipt, parent_head_data_hash, pov, status: CollationStatus::Created },
	);

	// It's collation-producer responsibility to verify that there exists
	// a hypothetical membership in a fragment tree for candidate.
	let interested: Vec<PeerId> = state
		.peer_views
		.iter()
		.filter(|(_, v)| match relay_parent_mode {
			ProspectiveParachainsMode::Disabled => v.contains(&candidate_relay_parent),
			ProspectiveParachainsMode::Enabled => v.iter().any(|block_hash| {
				state
					.implicit_view
					.known_allowed_relay_parents_under(block_hash, Some(id))
					.unwrap_or_default()
					.contains(&candidate_relay_parent)
			}),
		})
		.map(|(peer, _)| *peer)
		.collect();

	// Make sure already connected peers get collations:
	for peer_id in interested {
		advertise_collation(
			ctx,
			candidate_relay_parent,
			per_relay_parent,
			&peer_id,
			&state.peer_ids,
			&state.metrics,
		)
		.await;
	}

	Ok(())
}

/// Get the Id of the Core that is assigned to the para being collated on if any
/// and the total number of cores.
async fn determine_core(
	sender: &mut impl overseer::SubsystemSender<RuntimeApiMessage>,
	para_id: ParaId,
	relay_parent: Hash,
	relay_parent_mode: ProspectiveParachainsMode,
) -> Result<Option<(CoreIndex, usize)>> {
	let cores = get_availability_cores(sender, relay_parent).await?;

	for (idx, core) in cores.iter().enumerate() {
		let core_para_id = match core {
			CoreState::Scheduled(scheduled) => Some(scheduled.para_id),
			CoreState::Occupied(occupied) =>
				if relay_parent_mode.is_enabled() {
					// With async backing we don't care about the core state,
					// it is only needed for figuring our validators group.
					Some(occupied.candidate_descriptor.para_id)
				} else {
					None
				},
			CoreState::Free => None,
		};

		if core_para_id == Some(para_id) {
			return Ok(Some(((idx as u32).into(), cores.len())))
		}
	}

	Ok(None)
}

/// Validators of a particular group index.
#[derive(Debug)]
struct GroupValidators {
	/// The validators of above group (their discovery keys).
	validators: Vec<AuthorityDiscoveryId>,
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
	let current_validators = groups
		.get(current_group_index.0 as usize)
		.map(|v| v.as_slice())
		.unwrap_or_default();

	let validators = &info.discovery_keys;

	let current_validators =
		current_validators.iter().map(|i| validators[i.0 as usize].clone()).collect();

	let current_validators = GroupValidators { validators: current_validators };

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

		ctx.send_message(NetworkBridgeMessage::SendCollationMessage(
			vec![peer],
			Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(wire_message)),
		))
		.await;
	}
}

/// Issue a connection request to a set of validators and
/// revoke the previous connection request.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn connect_to_validators<Context>(
	ctx: &mut Context,
	validator_ids: Vec<AuthorityDiscoveryId>,
) {
	// ignore address resolution failure
	// will reissue a new request on new collation
	let (failed, _) = oneshot::channel();
	ctx.send_message(NetworkBridgeMessage::ConnectToValidators {
		validator_ids,
		peer_set: PeerSet::Collation,
		failed,
	})
	.await;
}

/// Advertise collation to the given `peer`.
///
/// This will only advertise a collation if there exists one for the given `relay_parent` and the given `peer` is
/// set as validator for our para at the given `relay_parent`.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn advertise_collation<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	per_relay_parent: &mut PerRelayParent,
	peer: &PeerId,
	peer_ids: &HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
	metrics: &Metrics,
) {
	for (candidate_hash, collation) in per_relay_parent.collations.iter_mut() {
		let should_advertise =
			per_relay_parent
				.validator_group
				.should_advertise_to(candidate_hash, peer_ids, &peer);

		if !should_advertise {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				peer_id = %peer,
				"Not advertising collation since validator is not interested",
			);
			continue
		}

		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			peer_id = %peer,
			"Advertising collation.",
		);
		collation.status.advance_to_advertised();

		let collation_message = match per_relay_parent.prospective_parachains_mode {
			ProspectiveParachainsMode::Enabled => {
				let wire_message = protocol_vstaging::CollatorProtocolMessage::AdvertiseCollation {
					relay_parent,
					candidate_hash: *candidate_hash,
					parent_head_data_hash: collation.parent_head_data_hash,
				};
				Versioned::VStaging(protocol_vstaging::CollationProtocol::CollatorProtocol(
					wire_message,
				))
			},
			ProspectiveParachainsMode::Disabled => {
				let wire_message =
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(relay_parent);
				Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(wire_message))
			},
		};

		ctx.send_message(NetworkBridgeMessage::SendCollationMessage(
			vec![peer.clone()],
			collation_message,
		))
		.await;

		per_relay_parent
			.validator_group
			.advertised_to_peer(candidate_hash, &peer_ids, peer);

		metrics.on_advertisment_made();
	}
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
		DistributeCollation(receipt, parent_head_data_hash, pov, result_sender) => {
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
					distribute_collation(
						ctx,
						runtime,
						state,
						id,
						receipt,
						parent_head_data_hash,
						pov,
						result_sender,
					)
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
	request: VersionedCollationRequest,
	receipt: CandidateReceipt,
	pov: PoV,
) {
	let (tx, rx) = oneshot::channel();

	let relay_parent = request.relay_parent();
	let peer_id = request.peer_id();
	let candidate_hash = receipt.hash();

	let response = OutgoingResponse {
		result: Ok(request_v1::CollationFetchingResponse::Collation(receipt, pov)),
		reputation_changes: Vec::new(),
		sent_feedback: Some(tx),
	};

	if let Err(_) = request.send_outgoing_response(response) {
		gum::warn!(target: LOG_TARGET, "Sending collation response failed");
	}

	state.active_collation_fetches.push(
		async move {
			let r = rx.timeout(MAX_UNSHARED_UPLOAD_TIME).await;
			if r.is_none() {
				gum::debug!(
					target: LOG_TARGET,
					?relay_parent,
					?peer_id,
					"Sending collation to validator timed out, carrying on with next validator."
				);
			}
			(relay_parent, candidate_hash, peer_id)
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
	msg: Versioned<
		protocol_v1::CollatorProtocolMessage,
		protocol_vstaging::CollatorProtocolMessage,
	>,
) -> Result<()> {
	use protocol_v1::CollatorProtocolMessage as V1;
	use protocol_vstaging::CollatorProtocolMessage as VStaging;

	match msg {
		Versioned::V1(V1::Declare(..)) | Versioned::VStaging(VStaging::Declare(..)) => {
			gum::trace!(
				target: LOG_TARGET,
				?origin,
				"Declare message is not expected on the collator side of the protocol",
			);

			// If we are declared to, this is another collator, and we should disconnect.
			ctx.send_message(NetworkBridgeMessage::DisconnectPeer(origin, PeerSet::Collation))
				.await;
		},
		Versioned::V1(V1::AdvertiseCollation(_)) |
		Versioned::VStaging(VStaging::AdvertiseCollation { .. }) => {
			gum::trace!(
				target: LOG_TARGET,
				?origin,
				"AdvertiseCollation message is not expected on the collator side of the protocol",
			);

			ctx.send_message(NetworkBridgeMessage::ReportPeer(
				origin.clone(),
				COST_UNEXPECTED_MESSAGE,
			))
			.await;

			// If we are advertised to, this is another collator, and we should disconnect.
			ctx.send_message(NetworkBridgeMessage::DisconnectPeer(origin, PeerSet::Collation))
				.await;
		},
		Versioned::V1(V1::CollationSeconded(relay_parent, statement)) |
		Versioned::VStaging(VStaging::CollationSeconded(relay_parent, statement)) => {
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
	req: std::result::Result<VersionedCollationRequest, incoming::Error>,
) -> Result<()> {
	let req = req?;
	let relay_parent = req.relay_parent();
	let peer_id = req.peer_id();
	let para_id = req.para_id();

	let _span = state
		.span_per_relay_parent
		.get(&relay_parent)
		.map(|s| s.child("request-collation"));

	match state.collating_on {
		Some(our_para_id) if our_para_id == para_id => {
			let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
				Some(per_relay_parent) => per_relay_parent,
				None => {
					gum::debug!(
						target: LOG_TARGET,
						relay_parent = %relay_parent,
						"received a `RequestCollation` for a relay parent out of our view",
					);

					return Ok(())
				},
			};

			let collation = match (per_relay_parent.prospective_parachains_mode, &req) {
				(ProspectiveParachainsMode::Disabled, VersionedCollationRequest::V1(_)) =>
					per_relay_parent.collations.values_mut().next(),
				(ProspectiveParachainsMode::Enabled, VersionedCollationRequest::VStaging(req)) =>
					per_relay_parent.collations.get_mut(&req.payload.candidate_hash),
				_ => {
					gum::warn!(
						target: LOG_TARGET,
						relay_parent = %relay_parent,
						mode = ?per_relay_parent.prospective_parachains_mode,
						"Collation request version is invalid",
					);

					return Ok(())
				},
			};
			let (receipt, pov) = if let Some(collation) = collation {
				collation.status.advance_to_requested();
				(collation.receipt.clone(), collation.pov.clone())
			} else {
				gum::warn!(
					target: LOG_TARGET,
					relay_parent = %relay_parent,
					"received a `RequestCollation` for a relay parent we don't have collation stored.",
				);

				return Ok(())
			};

			state.metrics.on_collation_sent_requested();

			let _span = _span.as_ref().map(|s| s.child("sending"));

			let waiting = state.waiting_collation_fetches.entry(relay_parent).or_default();
			let candidate_hash = receipt.hash();

			if !waiting.waiting_peers.insert((peer_id, candidate_hash)) {
				gum::debug!(
					target: LOG_TARGET,
					"Dropping incoming request as peer has a request in flight already."
				);
				ctx.send_message(NetworkBridgeMessage::ReportPeer(peer_id, COST_APPARENT_FLOOD))
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
				for_para_id = %para_id,
				our_para_id = %our_para_id,
				"received a `CollationFetchingRequest` for unexpected para_id",
			);
		},
		None => {
			gum::warn!(
				target: LOG_TARGET,
				for_para_id = %para_id,
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
	let current = state.peer_views.entry(peer_id.clone()).or_default();

	let added: Vec<Hash> = view.difference(&*current).cloned().collect();

	*current = view;

	for added in added.into_iter() {
		let block_hashes = match state
			.per_relay_parent
			.get(&added)
			.map(|per_relay_parent| per_relay_parent.prospective_parachains_mode)
		{
			Some(ProspectiveParachainsMode::Disabled) => std::slice::from_ref(&added),
			Some(ProspectiveParachainsMode::Enabled) => state
				.implicit_view
				.known_allowed_relay_parents_under(&added, state.collating_on)
				.unwrap_or_default(),
			None => {
				// Added leaf is unknown.
				continue
			},
		};

		for block_hash in block_hashes {
			let per_relay_parent = match state.per_relay_parent.get_mut(block_hash) {
				Some(per_relay_parent) => per_relay_parent,
				None => continue,
			};
			advertise_collation(
				ctx,
				*block_hash,
				per_relay_parent,
				&peer_id,
				&state.peer_ids,
				&state.metrics,
			)
			.await;
		}
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
			handle_our_view_change(ctx.sender(), state, view).await?;
		},
		PeerMessage(remote, msg) => {
			handle_incoming_peer_message(ctx, runtime, state, remote, msg).await?;
		},
		NewGossipTopology { .. } => {
			// impossible!
		},
	}

	Ok(())
}

/// Handles our view changes.
async fn handle_our_view_change<Sender>(
	sender: &mut Sender,
	state: &mut State,
	view: OurView,
) -> Result<()>
where
	Sender: CollatorProtocolSenderTrait,
{
	let current_leaves = state.active_leaves.clone();

	let removed = current_leaves.iter().filter(|(h, _)| !view.contains(*h));
	let added = view.iter().filter(|h| !current_leaves.contains_key(h));

	for leaf in added {
		let mode = prospective_parachains_mode(sender, *leaf).await?;

		if let Some(span) = view.span_per_head().get(leaf).cloned() {
			let per_leaf_span = PerLeafSpan::new(span, "collator-side");
			state.span_per_relay_parent.insert(*leaf, per_leaf_span);
		}

		state.active_leaves.insert(*leaf, mode);
		state.per_relay_parent.insert(*leaf, PerRelayParent::new(mode));

		if mode.is_enabled() {
			state
				.implicit_view
				.activate_leaf(sender, *leaf)
				.await
				.map_err(Error::ImplicitViewFetchError)?;

			let allowed_ancestry = state
				.implicit_view
				.known_allowed_relay_parents_under(leaf, state.collating_on)
				.unwrap_or_default();
			for block_hash in allowed_ancestry {
				state
					.per_relay_parent
					.entry(*block_hash)
					.or_insert_with(|| PerRelayParent::new(ProspectiveParachainsMode::Enabled));
			}
		}
	}

	for (leaf, mode) in removed {
		state.active_leaves.remove(leaf);
		// If the leaf is deactivated it still may stay in the view as a part
		// of implicit ancestry. Only update the state after the hash is actually
		// pruned from the block info storage.
		let pruned = if mode.is_enabled() {
			state.implicit_view.deactivate_leaf(*leaf)
		} else {
			vec![*leaf]
		};

		for removed in &pruned {
			gum::debug!(target: LOG_TARGET, relay_parent = ?removed, "Removing relay parent because our view changed.");

			let collations = state
				.per_relay_parent
				.get_mut(removed)
				.map(|per_relay_parent| std::mem::take(&mut per_relay_parent.collations))
				.unwrap_or_default();
			for collation in collations.into_values() {
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
			state.per_relay_parent.remove(removed);
			state.span_per_relay_parent.remove(removed);
			state.waiting_collation_fetches.remove(removed);
		}
	}
	Ok(())
}

/// The collator protocol collator side main loop.
#[overseer::contextbounds(CollatorProtocol, prefix = crate::overseer)]
pub(crate) async fn run<Context>(
	mut ctx: Context,
	local_peer_id: PeerId,
	collator_pair: CollatorPair,
	mut req_v1_receiver: IncomingRequestReceiver<request_v1::CollationFetchingRequest>,
	mut req_v2_receiver: IncomingRequestReceiver<request_vstaging::CollationFetchingRequest>,
	metrics: Metrics,
) -> std::result::Result<(), FatalError> {
	use OverseerSignal::*;

	let mut state = State::new(local_peer_id, collator_pair, metrics);
	let mut runtime = RuntimeInfo::new(None);

	loop {
		let reputation_changes = || vec![COST_INVALID_REQUEST];
		let recv_req_v1 = req_v1_receiver.recv(reputation_changes).fuse();
		let recv_req_v2 = req_v2_receiver.recv(reputation_changes).fuse();
		pin_mut!(recv_req_v1);
		pin_mut!(recv_req_v2);

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
			(relay_parent, candidate_hash, peer_id) = state.active_collation_fetches.select_next_some() => {
				let next = if let Some(waiting) = state.waiting_collation_fetches.get_mut(&relay_parent) {
					waiting.waiting_peers.remove(&(peer_id, candidate_hash));
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

				let next_collation = {
					let per_relay_parent = match state.per_relay_parent.get(&relay_parent) {
						Some(per_relay_parent) => per_relay_parent,
						None => continue,
					};

					match (per_relay_parent.prospective_parachains_mode, &next) {
						(ProspectiveParachainsMode::Disabled, VersionedCollationRequest::V1(_)) => {
							per_relay_parent.collations.values().next()
						},
						(ProspectiveParachainsMode::Enabled, VersionedCollationRequest::VStaging(req)) => {
							per_relay_parent.collations.get(&req.payload.candidate_hash)
						},
						_ => continue,
					}
				};

				if let Some(collation) = next_collation {
					let receipt = collation.receipt.clone();
					let pov = collation.pov.clone();

					send_collation(&mut state, next, receipt, pov).await;
				}
			}
			in_req = recv_req_v1 => {
				let request = in_req.map(VersionedCollationRequest::from);

				log_error(
					handle_incoming_request(&mut ctx, &mut state, request).await,
					"Handling incoming collation fetch request V1"
				)?;
			}
			in_req = recv_req_v2 => {
				let request = in_req.map(VersionedCollationRequest::from);

				log_error(
					handle_incoming_request(&mut ctx, &mut state, request).await,
					"Handling incoming collation fetch request VStaging"
				)?;
			}
		}
	}
}
