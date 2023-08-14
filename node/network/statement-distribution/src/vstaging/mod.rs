// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Implementation of the v2 statement distribution protocol,
//! designed for asynchronous backing.

use polkadot_node_network_protocol::{
	self as net_protocol,
	grid_topology::SessionGridTopology,
	peer_set::ValidationVersion,
	request_response::{
		incoming::OutgoingResponse,
		vstaging::{AttestedCandidateRequest, AttestedCandidateResponse},
		IncomingRequest, IncomingRequestReceiver, Requests,
		MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS,
	},
	vstaging::{self as protocol_vstaging, StatementFilter},
	IfDisconnected, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{
	SignedFullStatementWithPVD, StatementWithPVD as FullStatementWithPVD,
};
use polkadot_node_subsystem::{
	messages::{
		CandidateBackingMessage, HypotheticalCandidate, HypotheticalFrontierRequest,
		NetworkBridgeEvent, NetworkBridgeTxMessage, ProspectiveParachainsMessage,
	},
	overseer, ActivatedLeaf,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::View as ImplicitView, reputation::ReputationAggregator,
	runtime::ProspectiveParachainsMode,
};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, CompactStatement, CoreIndex, CoreState, GroupIndex,
	GroupRotationInfo, Hash, Id as ParaId, IndexedVec, SessionIndex, SessionInfo, SignedStatement,
	SigningContext, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
};

use sp_keystore::KeystorePtr;

use fatality::Nested;
use futures::{
	channel::{mpsc, oneshot},
	stream::FuturesUnordered,
	SinkExt, StreamExt,
};

use std::{
	collections::{
		hash_map::{Entry, HashMap},
		HashSet,
	},
	time::{Duration, Instant},
};

use crate::{
	error::{JfyiError, JfyiErrorResult},
	LOG_TARGET,
};
use candidates::{BadAdvertisement, Candidates, PostConfirmation};
use cluster::{Accept as ClusterAccept, ClusterTracker, RejectIncoming as ClusterRejectIncoming};
use grid::GridTracker;
use groups::Groups;
use requests::{CandidateIdentifier, RequestProperties};
use statement_store::{StatementOrigin, StatementStore};

pub use requests::{RequestManager, ResponseManager, UnhandledResponse};

mod candidates;
mod cluster;
mod grid;
mod groups;
mod requests;
mod statement_store;

#[cfg(test)]
mod tests;

const COST_UNEXPECTED_STATEMENT: Rep = Rep::CostMinor("Unexpected Statement");
const COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE: Rep =
	Rep::CostMinor("Unexpected Statement, missing knowledge for relay parent");
const COST_EXCESSIVE_SECONDED: Rep = Rep::CostMinor("Sent Excessive `Seconded` Statements");

const COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE: Rep =
	Rep::CostMinor("Unexpected Manifest, missing knowlege for relay parent");
const COST_UNEXPECTED_MANIFEST_DISALLOWED: Rep =
	Rep::CostMinor("Unexpected Manifest, Peer Disallowed");
const COST_CONFLICTING_MANIFEST: Rep = Rep::CostMajor("Manifest conflicts with previous");
const COST_INSUFFICIENT_MANIFEST: Rep =
	Rep::CostMajor("Manifest statements insufficient to back candidate");
const COST_MALFORMED_MANIFEST: Rep = Rep::CostMajor("Manifest is malformed");
const COST_UNEXPECTED_ACKNOWLEDGEMENT_UNKNOWN_CANDIDATE: Rep =
	Rep::CostMinor("Unexpected acknowledgement, unknown candidate");

const COST_INVALID_SIGNATURE: Rep = Rep::CostMajor("Invalid Statement Signature");
const COST_IMPROPERLY_DECODED_RESPONSE: Rep =
	Rep::CostMajor("Improperly Encoded Candidate Response");
const COST_INVALID_RESPONSE: Rep = Rep::CostMajor("Invalid Candidate Response");
const COST_UNREQUESTED_RESPONSE_STATEMENT: Rep =
	Rep::CostMajor("Un-requested Statement In Response");
const COST_INACCURATE_ADVERTISEMENT: Rep =
	Rep::CostMajor("Peer advertised a candidate inaccurately");

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Peer sent unparsable request");
const COST_INVALID_REQUEST_BITFIELD_SIZE: Rep =
	Rep::CostMajor("Attested candidate request bitfields have wrong size");
const COST_UNEXPECTED_REQUEST: Rep = Rep::CostMajor("Unexpected attested candidate request");

const BENEFIT_VALID_RESPONSE: Rep = Rep::BenefitMajor("Peer Answered Candidate Request");
const BENEFIT_VALID_STATEMENT: Rep = Rep::BenefitMajor("Peer provided a valid statement");
const BENEFIT_VALID_STATEMENT_FIRST: Rep =
	Rep::BenefitMajorFirst("Peer was the first to provide a given valid statement");

/// The amount of time to wait before retrying when the node sends a request and it is dropped.
pub(crate) const REQUEST_RETRY_DELAY: Duration = Duration::from_secs(1);

struct PerRelayParentState {
	local_validator: Option<LocalValidatorState>,
	statement_store: StatementStore,
	availability_cores: Vec<CoreState>,
	group_rotation_info: GroupRotationInfo,
	seconding_limit: usize,
	session: SessionIndex,
}

// per-relay-parent local validator state.
struct LocalValidatorState {
	// The index of the validator.
	index: ValidatorIndex,
	// our validator group
	group: GroupIndex,
	// the assignment of our validator group, if any.
	assignment: Option<ParaId>,
	// the 'direct-in-group' communication at this relay-parent.
	cluster_tracker: ClusterTracker,
	// the grid-level communication at this relay-parent.
	grid_tracker: GridTracker,
}

#[derive(Debug)]
struct PerSessionState {
	session_info: SessionInfo,
	groups: Groups,
	authority_lookup: HashMap<AuthorityDiscoveryId, ValidatorIndex>,
	// is only `None` in the time between seeing a session and
	// getting the topology from the gossip-support subsystem
	grid_view: Option<grid::SessionTopologyView>,
	local_validator: Option<ValidatorIndex>,
}

impl PerSessionState {
	fn new(session_info: SessionInfo, keystore: &KeystorePtr) -> Self {
		let groups = Groups::new(session_info.validator_groups.clone());
		let mut authority_lookup = HashMap::new();
		for (i, ad) in session_info.discovery_keys.iter().cloned().enumerate() {
			authority_lookup.insert(ad, ValidatorIndex(i as _));
		}

		let local_validator = polkadot_node_subsystem_util::signing_key_and_index(
			session_info.validators.iter(),
			keystore,
		);

		PerSessionState {
			session_info,
			groups,
			authority_lookup,
			grid_view: None,
			local_validator: local_validator.map(|(_key, index)| index),
		}
	}

	fn supply_topology(&mut self, topology: &SessionGridTopology) {
		let grid_view = grid::build_session_topology(
			self.session_info.validator_groups.iter(),
			topology,
			self.local_validator,
		);

		self.grid_view = Some(grid_view);
	}
}

pub(crate) struct State {
	/// The utility for managing the implicit and explicit views in a consistent way.
	///
	/// We only feed leaves which have prospective parachains enabled to this view.
	implicit_view: ImplicitView,
	candidates: Candidates,
	per_relay_parent: HashMap<Hash, PerRelayParentState>,
	per_session: HashMap<SessionIndex, PerSessionState>,
	peers: HashMap<PeerId, PeerState>,
	keystore: KeystorePtr,
	authorities: HashMap<AuthorityDiscoveryId, PeerId>,
	request_manager: RequestManager,
	response_manager: ResponseManager,
}

impl State {
	/// Create a new state.
	pub(crate) fn new(keystore: KeystorePtr) -> Self {
		State {
			implicit_view: Default::default(),
			candidates: Default::default(),
			per_relay_parent: HashMap::new(),
			per_session: HashMap::new(),
			peers: HashMap::new(),
			keystore,
			authorities: HashMap::new(),
			request_manager: RequestManager::new(),
			response_manager: ResponseManager::new(),
		}
	}

	pub(crate) fn request_and_response_managers(
		&mut self,
	) -> (&mut RequestManager, &mut ResponseManager) {
		(&mut self.request_manager, &mut self.response_manager)
	}
}

// For the provided validator index, if there is a connected peer controlling the given authority
// ID, then return that peer's `PeerId`.
fn connected_validator_peer(
	authorities: &HashMap<AuthorityDiscoveryId, PeerId>,
	per_session: &PerSessionState,
	validator_index: ValidatorIndex,
) -> Option<PeerId> {
	per_session
		.session_info
		.discovery_keys
		.get(validator_index.0 as usize)
		.and_then(|k| authorities.get(k))
		.map(|p| *p)
}

struct PeerState {
	view: View,
	implicit_view: HashSet<Hash>,
	discovery_ids: Option<HashSet<AuthorityDiscoveryId>>,
}

impl PeerState {
	// Update the view, returning a vector of implicit relay-parents which weren't previously
	// part of the view.
	fn update_view(&mut self, new_view: View, local_implicit: &ImplicitView) -> Vec<Hash> {
		let next_implicit = new_view
			.iter()
			.flat_map(|x| local_implicit.known_allowed_relay_parents_under(x, None))
			.flatten()
			.cloned()
			.collect::<HashSet<_>>();

		let fresh_implicit = next_implicit
			.iter()
			.filter(|x| !self.implicit_view.contains(x))
			.cloned()
			.collect();

		self.view = new_view;
		self.implicit_view = next_implicit;

		fresh_implicit
	}

	// Attempt to reconcile the view with new information about the implicit relay parents
	// under an active leaf.
	fn reconcile_active_leaf(&mut self, leaf_hash: Hash, implicit: &[Hash]) -> Vec<Hash> {
		if !self.view.contains(&leaf_hash) {
			return Vec::new()
		}

		let mut v = Vec::with_capacity(implicit.len());
		for i in implicit {
			if self.implicit_view.insert(*i) {
				v.push(*i);
			}
		}
		v
	}

	// Whether we know that a peer knows a relay-parent.
	// The peer knows the relay-parent if it is either implicit or explicit
	// in their view. However, if it is implicit via an active-leaf we don't
	// recognize, we will not accurately be able to recognize them as 'knowing'
	// the relay-parent.
	fn knows_relay_parent(&self, relay_parent: &Hash) -> bool {
		self.implicit_view.contains(relay_parent) || self.view.contains(relay_parent)
	}

	fn is_authority(&self, authority_id: &AuthorityDiscoveryId) -> bool {
		self.discovery_ids.as_ref().map_or(false, |x| x.contains(authority_id))
	}

	fn iter_known_discovery_ids(&self) -> impl Iterator<Item = &AuthorityDiscoveryId> {
		self.discovery_ids.as_ref().into_iter().flatten()
	}
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_network_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: NetworkBridgeEvent<net_protocol::StatementDistributionMessage>,
	reputation: &mut ReputationAggregator,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer_id, role, protocol_version, mut authority_ids) => {
			gum::trace!(target: LOG_TARGET, ?peer_id, ?role, ?protocol_version, "Peer connected");

			if protocol_version != ValidationVersion::VStaging.into() {
				return
			}

			if let Some(ref mut authority_ids) = authority_ids {
				authority_ids.retain(|a| match state.authorities.entry(a.clone()) {
					Entry::Vacant(e) => {
						e.insert(peer_id);
						true
					},
					Entry::Occupied(e) => {
						gum::trace!(
							target: LOG_TARGET,
							authority_id = ?a,
							existing_peer = ?e.get(),
							new_peer = ?peer_id,
							"Ignoring new peer with duplicate authority ID as a bearer of that identity"
						);

						false
					},
				});
			}

			state.peers.insert(
				peer_id,
				PeerState {
					view: View::default(),
					implicit_view: HashSet::new(),
					discovery_ids: authority_ids,
				},
			);
		},
		NetworkBridgeEvent::PeerDisconnected(peer_id) => {
			if let Some(p) = state.peers.remove(&peer_id) {
				for discovery_key in p.discovery_ids.into_iter().flatten() {
					state.authorities.remove(&discovery_key);
				}
			}
		},
		NetworkBridgeEvent::NewGossipTopology(topology) => {
			let new_session_index = topology.session;
			let new_topology = topology.topology;

			if let Some(per_session) = state.per_session.get_mut(&new_session_index) {
				per_session.supply_topology(&new_topology);
			}

			// TODO [https://github.com/paritytech/polkadot/issues/6194]
			// technically, we should account for the fact that the session topology might
			// come late, and for all relay-parents with this session, send all grid peers
			// any `BackedCandidateInv` messages they might need.
			//
			// in practice, this is a small issue & the API of receiving topologies could
			// be altered to fix it altogether.
		},
		NetworkBridgeEvent::PeerMessage(peer_id, message) => match message {
			net_protocol::StatementDistributionMessage::V1(_) => return,
			net_protocol::StatementDistributionMessage::VStaging(
				protocol_vstaging::StatementDistributionMessage::V1Compatibility(_),
			) => return,
			net_protocol::StatementDistributionMessage::VStaging(
				protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, statement),
			) =>
				handle_incoming_statement(ctx, state, peer_id, relay_parent, statement, reputation)
					.await,
			net_protocol::StatementDistributionMessage::VStaging(
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(inner),
			) => handle_incoming_manifest(ctx, state, peer_id, inner, reputation).await,
			net_protocol::StatementDistributionMessage::VStaging(
				protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(inner),
			) => handle_incoming_acknowledgement(ctx, state, peer_id, inner, reputation).await,
		},
		NetworkBridgeEvent::PeerViewChange(peer_id, view) =>
			handle_peer_view_update(ctx, state, peer_id, view).await,
		NetworkBridgeEvent::OurViewChange(_view) => {
			// handled by `handle_activated_leaf`
		},
		NetworkBridgeEvent::UpdatedAuthorityIds(peer_id, authority_ids) => {
			gum::trace!(
				target: LOG_TARGET,
				?peer_id,
				?authority_ids,
				"Updated `AuthorityDiscoveryId`s"
			);

			// Remove the authority IDs which were previously mapped to the peer
			// but aren't part of the new set.
			state.authorities.retain(|a, p| p != &peer_id || authority_ids.contains(a));

			// Map the new authority IDs to the peer.
			for a in authority_ids.iter().cloned() {
				state.authorities.insert(a, peer_id);
			}

			if let Some(peer_state) = state.peers.get_mut(&peer_id) {
				peer_state.discovery_ids = Some(authority_ids);
			}
		},
	}
}

/// If there is a new leaf, this should only be called for leaves which support
/// prospective parachains.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	activated: &ActivatedLeaf,
	leaf_mode: ProspectiveParachainsMode,
) -> JfyiErrorResult<()> {
	let seconding_limit = match leaf_mode {
		ProspectiveParachainsMode::Disabled => return Ok(()),
		ProspectiveParachainsMode::Enabled { max_candidate_depth, .. } => max_candidate_depth + 1,
	};

	state
		.implicit_view
		.activate_leaf(ctx.sender(), activated.hash)
		.await
		.map_err(JfyiError::ActivateLeafFailure)?;

	let new_relay_parents =
		state.implicit_view.all_allowed_relay_parents().cloned().collect::<Vec<_>>();
	for new_relay_parent in new_relay_parents.iter().cloned() {
		if state.per_relay_parent.contains_key(&new_relay_parent) {
			continue
		}

		// New leaf: fetch info from runtime API and initialize
		// `per_relay_parent`.

		let session_index = polkadot_node_subsystem_util::request_session_index_for_child(
			new_relay_parent,
			ctx.sender(),
		)
		.await
		.await
		.map_err(JfyiError::RuntimeApiUnavailable)?
		.map_err(JfyiError::FetchSessionIndex)?;

		let availability_cores = polkadot_node_subsystem_util::request_availability_cores(
			new_relay_parent,
			ctx.sender(),
		)
		.await
		.await
		.map_err(JfyiError::RuntimeApiUnavailable)?
		.map_err(JfyiError::FetchAvailabilityCores)?;

		let group_rotation_info =
			polkadot_node_subsystem_util::request_validator_groups(new_relay_parent, ctx.sender())
				.await
				.await
				.map_err(JfyiError::RuntimeApiUnavailable)?
				.map_err(JfyiError::FetchValidatorGroups)?
				.1;

		if !state.per_session.contains_key(&session_index) {
			let session_info = polkadot_node_subsystem_util::request_session_info(
				new_relay_parent,
				session_index,
				ctx.sender(),
			)
			.await
			.await
			.map_err(JfyiError::RuntimeApiUnavailable)?
			.map_err(JfyiError::FetchSessionInfo)?;

			let session_info = match session_info {
				None => {
					gum::warn!(
						target: LOG_TARGET,
						relay_parent = ?new_relay_parent,
						"No session info available for current session"
					);

					continue
				},
				Some(s) => s,
			};

			state
				.per_session
				.insert(session_index, PerSessionState::new(session_info, &state.keystore));
		}

		let per_session = state
			.per_session
			.get(&session_index)
			.expect("either existed or just inserted; qed");

		let local_validator = per_session.local_validator.and_then(|v| {
			find_local_validator_state(
				v,
				&per_session.groups,
				&availability_cores,
				&group_rotation_info,
				seconding_limit,
			)
		});

		state.per_relay_parent.insert(
			new_relay_parent,
			PerRelayParentState {
				local_validator,
				statement_store: StatementStore::new(&per_session.groups),
				availability_cores,
				group_rotation_info,
				seconding_limit,
				session: session_index,
			},
		);
	}

	// Reconcile all peers' views with the active leaf and any relay parents
	// it implies. If they learned about the block before we did, this reconciliation will give
	// non-empty results and we should send them messages concerning all activated relay-parents.
	{
		let mut update_peers = Vec::new();
		for (peer, peer_state) in state.peers.iter_mut() {
			let fresh = peer_state.reconcile_active_leaf(activated.hash, &new_relay_parents);
			if !fresh.is_empty() {
				update_peers.push((*peer, fresh));
			}
		}

		for (peer, fresh) in update_peers {
			for fresh_relay_parent in fresh {
				send_peer_messages_for_relay_parent(ctx, state, peer, fresh_relay_parent).await;
			}
		}
	}

	new_leaf_fragment_tree_updates(ctx, state, activated.hash).await;

	Ok(())
}

fn find_local_validator_state(
	validator_index: ValidatorIndex,
	groups: &Groups,
	availability_cores: &[CoreState],
	group_rotation_info: &GroupRotationInfo,
	seconding_limit: usize,
) -> Option<LocalValidatorState> {
	if groups.all().is_empty() {
		return None
	}

	let our_group = groups.by_validator_index(validator_index)?;

	// note: this won't work well for on-demand parachains because it only works
	// when core assignments to paras are static throughout the session.

	let core = group_rotation_info.core_for_group(our_group, availability_cores.len());
	let para = availability_cores.get(core.0 as usize).and_then(|c| c.para_id());
	let group_validators = groups.get(our_group)?.to_owned();

	Some(LocalValidatorState {
		index: validator_index,
		group: our_group,
		assignment: para,
		cluster_tracker: ClusterTracker::new(group_validators, seconding_limit)
			.expect("group is non-empty because we are in it; qed"),
		grid_tracker: GridTracker::default(),
	})
}

pub(crate) fn handle_deactivate_leaves(state: &mut State, leaves: &[Hash]) {
	// deactivate the leaf in the implicit view.
	for leaf in leaves {
		state.implicit_view.deactivate_leaf(*leaf);
	}

	let relay_parents = state.implicit_view.all_allowed_relay_parents().collect::<HashSet<_>>();

	// fast exit for no-op.
	if relay_parents.len() == state.per_relay_parent.len() {
		return
	}

	// clean up per-relay-parent data based on everything removed.
	state.per_relay_parent.retain(|r, _| relay_parents.contains(r));

	// Clean up all requests
	for leaf in leaves {
		state.request_manager.remove_by_relay_parent(*leaf);
	}

	state.candidates.on_deactivate_leaves(&leaves, |h| relay_parents.contains(h));

	// clean up sessions based on everything remaining.
	let sessions: HashSet<_> = state.per_relay_parent.values().map(|r| r.session).collect();
	state.per_session.retain(|s, _| sessions.contains(s));
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_peer_view_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	new_view: View,
) {
	let fresh_implicit = {
		let peer_data = match state.peers.get_mut(&peer) {
			None => return,
			Some(p) => p,
		};

		peer_data.update_view(new_view, &state.implicit_view)
	};

	for new_relay_parent in fresh_implicit {
		send_peer_messages_for_relay_parent(ctx, state, peer, new_relay_parent).await;
	}
}

// Returns an iterator over known validator indices, given an iterator over discovery IDs
// and a mapping from discovery IDs to validator indices.
fn find_validator_ids<'a>(
	known_discovery_ids: impl IntoIterator<Item = &'a AuthorityDiscoveryId>,
	discovery_mapping: impl Fn(&AuthorityDiscoveryId) -> Option<&'a ValidatorIndex>,
) -> impl Iterator<Item = ValidatorIndex> {
	known_discovery_ids.into_iter().filter_map(discovery_mapping).cloned()
}

/// Send a peer, apparently just becoming aware of a relay-parent, all messages
/// concerning that relay-parent.
///
/// In particular, we send all statements pertaining to our common cluster,
/// as well as all manifests, acknowledgements, or other grid statements.
///
/// Note that due to the way we handle views, our knowledge of peers' relay parents
/// may "oscillate" with relay parents repeatedly leaving and entering the
/// view of a peer based on the implicit view of active leaves.
///
/// This function is designed to be cheap and not to send duplicate messages in repeated
/// cases.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_peer_messages_for_relay_parent<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	relay_parent: Hash,
) {
	let peer_data = match state.peers.get_mut(&peer) {
		None => return,
		Some(p) => p,
	};

	let relay_parent_state = match state.per_relay_parent.get_mut(&relay_parent) {
		None => return,
		Some(s) => s,
	};

	let per_session_state = match state.per_session.get(&relay_parent_state.session) {
		None => return,
		Some(s) => s,
	};

	for validator_id in find_validator_ids(peer_data.iter_known_discovery_ids(), |a| {
		per_session_state.authority_lookup.get(a)
	}) {
		if let Some(local_validator_state) = relay_parent_state.local_validator.as_mut() {
			send_pending_cluster_statements(
				ctx,
				relay_parent,
				&peer,
				validator_id,
				&mut local_validator_state.cluster_tracker,
				&state.candidates,
				&relay_parent_state.statement_store,
			)
			.await;
		}

		send_pending_grid_messages(
			ctx,
			relay_parent,
			&peer,
			validator_id,
			&per_session_state.groups,
			relay_parent_state,
			&state.candidates,
		)
		.await;
	}
}

fn pending_statement_network_message(
	statement_store: &StatementStore,
	relay_parent: Hash,
	peer: &PeerId,
	originator: ValidatorIndex,
	compact: CompactStatement,
) -> Option<(Vec<PeerId>, net_protocol::VersionedValidationProtocol)> {
	statement_store
		.validator_statement(originator, compact)
		.map(|s| s.as_unchecked().clone())
		.map(|signed| {
			protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, signed)
		})
		.map(|msg| (vec![*peer], Versioned::VStaging(msg).into()))
}

/// Send a peer all pending cluster statements for a relay parent.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_pending_cluster_statements<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	peer_id: &PeerId,
	peer_validator_id: ValidatorIndex,
	cluster_tracker: &mut ClusterTracker,
	candidates: &Candidates,
	statement_store: &StatementStore,
) {
	let pending_statements = cluster_tracker.pending_statements_for(peer_validator_id);
	let network_messages = pending_statements
		.into_iter()
		.filter_map(|(originator, compact)| {
			if !candidates.is_confirmed(compact.candidate_hash()) {
				return None
			}

			let res = pending_statement_network_message(
				&statement_store,
				relay_parent,
				peer_id,
				originator,
				compact.clone(),
			);

			if res.is_some() {
				cluster_tracker.note_sent(peer_validator_id, originator, compact);
			}

			res
		})
		.collect::<Vec<_>>();

	if network_messages.is_empty() {
		return
	}

	ctx.send_message(NetworkBridgeTxMessage::SendValidationMessages(network_messages))
		.await;
}

/// Send a peer all pending grid messages / acknowledgements / follow up statements
/// upon learning about a new relay parent.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_pending_grid_messages<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	peer_id: &PeerId,
	peer_validator_id: ValidatorIndex,
	groups: &Groups,
	relay_parent_state: &mut PerRelayParentState,
	candidates: &Candidates,
) {
	let pending_manifests = {
		let local_validator = match relay_parent_state.local_validator.as_mut() {
			None => return,
			Some(l) => l,
		};

		let grid_tracker = &mut local_validator.grid_tracker;
		grid_tracker.pending_manifests_for(peer_validator_id)
	};

	let mut messages: Vec<(Vec<PeerId>, net_protocol::VersionedValidationProtocol)> = Vec::new();
	for (candidate_hash, kind) in pending_manifests {
		let confirmed_candidate = match candidates.get_confirmed(&candidate_hash) {
			None => continue, // sanity
			Some(c) => c,
		};

		let group_index = confirmed_candidate.group_index();

		let local_knowledge = {
			let group_size = match groups.get(group_index) {
				None => return, // sanity
				Some(x) => x.len(),
			};

			local_knowledge_filter(
				group_size,
				group_index,
				candidate_hash,
				&relay_parent_state.statement_store,
			)
		};

		match kind {
			grid::ManifestKind::Full => {
				let manifest = protocol_vstaging::BackedCandidateManifest {
					relay_parent,
					candidate_hash,
					group_index,
					para_id: confirmed_candidate.para_id(),
					parent_head_data_hash: confirmed_candidate.parent_head_data_hash(),
					statement_knowledge: local_knowledge.clone(),
				};

				let grid = &mut relay_parent_state
					.local_validator
					.as_mut()
					.expect("determined to be some earlier in this function; qed")
					.grid_tracker;

				grid.manifest_sent_to(
					groups,
					peer_validator_id,
					candidate_hash,
					local_knowledge.clone(),
				);

				messages.push((
					vec![*peer_id],
					Versioned::VStaging(
						protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
							manifest,
						),
					)
					.into(),
				));
			},
			grid::ManifestKind::Acknowledgement => {
				messages.extend(acknowledgement_and_statement_messages(
					*peer_id,
					peer_validator_id,
					groups,
					relay_parent_state,
					relay_parent,
					group_index,
					candidate_hash,
					local_knowledge,
				));
			},
		}
	}

	// Send all remaining pending grid statements for a validator, not just
	// those for the acknowledgements we've sent.
	//
	// otherwise, we might receive statements while the grid peer is "out of view" and then
	// not send them when they get back "in view". problem!
	{
		let grid_tracker = &mut relay_parent_state
			.local_validator
			.as_mut()
			.expect("checked earlier; qed")
			.grid_tracker;

		let pending_statements = grid_tracker.all_pending_statements_for(peer_validator_id);

		let extra_statements =
			pending_statements.into_iter().filter_map(|(originator, compact)| {
				let res = pending_statement_network_message(
					&relay_parent_state.statement_store,
					relay_parent,
					peer_id,
					originator,
					compact.clone(),
				);

				if res.is_some() {
					grid_tracker.sent_or_received_direct_statement(
						groups,
						originator,
						peer_validator_id,
						&compact,
					);
				}

				res
			});

		messages.extend(extra_statements);
	}

	if messages.is_empty() {
		return
	}
	ctx.send_message(NetworkBridgeTxMessage::SendValidationMessages(messages)).await;
}

// Imports a locally originating statement and distributes it to peers.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn share_local_statement<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	statement: SignedFullStatementWithPVD,
	reputation: &mut ReputationAggregator,
) -> JfyiErrorResult<()> {
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		None => return Err(JfyiError::InvalidShare),
		Some(x) => x,
	};

	gum::debug!(
		target: LOG_TARGET,
		statement = ?statement.payload().to_compact(),
		"Sharing Statement",
	);

	let per_session = match state.per_session.get(&per_relay_parent.session) {
		Some(s) => s,
		None => return Ok(()),
	};

	let (local_index, local_assignment, local_group) =
		match per_relay_parent.local_validator.as_ref() {
			None => return Err(JfyiError::InvalidShare),
			Some(l) => (l.index, l.assignment, l.group),
		};

	// Two possibilities: either the statement is `Seconded` or we already
	// have the candidate. Sanity: check the para-id is valid.
	let expected = match statement.payload() {
		FullStatementWithPVD::Seconded(ref c, _) =>
			Some((c.descriptor().para_id, c.descriptor().relay_parent)),
		FullStatementWithPVD::Valid(hash) =>
			state.candidates.get_confirmed(&hash).map(|c| (c.para_id(), c.relay_parent())),
	};

	let is_seconded = match statement.payload() {
		FullStatementWithPVD::Seconded(_, _) => true,
		FullStatementWithPVD::Valid(_) => false,
	};

	let (expected_para, expected_relay_parent) = match expected {
		None => return Err(JfyiError::InvalidShare),
		Some(x) => x,
	};

	if local_index != statement.validator_index() {
		return Err(JfyiError::InvalidShare)
	}

	if is_seconded &&
		per_relay_parent.statement_store.seconded_count(&local_index) ==
			per_relay_parent.seconding_limit
	{
		gum::warn!(
			target: LOG_TARGET,
			limit = ?per_relay_parent.seconding_limit,
			"Local node has issued too many `Seconded` statements",
		);
		return Err(JfyiError::InvalidShare)
	}

	if local_assignment != Some(expected_para) || relay_parent != expected_relay_parent {
		return Err(JfyiError::InvalidShare)
	}

	let mut post_confirmation = None;

	// Insert candidate if unknown + more sanity checks.
	let compact_statement = {
		let compact_statement = FullStatementWithPVD::signed_to_compact(statement.clone());
		let candidate_hash = CandidateHash(*statement.payload().candidate_hash());

		if let FullStatementWithPVD::Seconded(ref c, ref pvd) = statement.payload() {
			post_confirmation = state.candidates.confirm_candidate(
				candidate_hash,
				c.clone(),
				pvd.clone(),
				local_group,
			);
		};

		match per_relay_parent.statement_store.insert(
			&per_session.groups,
			compact_statement.clone(),
			StatementOrigin::Local,
		) {
			Ok(false) | Err(_) => {
				gum::warn!(
					target: LOG_TARGET,
					statement = ?compact_statement.payload(),
					"Candidate backing issued redundant statement?",
				);
				return Err(JfyiError::InvalidShare)
			},
			Ok(true) => {},
		}

		{
			let l = per_relay_parent.local_validator.as_mut().expect("checked above; qed");
			l.cluster_tracker.note_issued(local_index, compact_statement.payload().clone());
		}

		if let Some(ref session_topology) = per_session.grid_view {
			let l = per_relay_parent.local_validator.as_mut().expect("checked above; qed");
			l.grid_tracker.learned_fresh_statement(
				&per_session.groups,
				session_topology,
				local_index,
				&compact_statement.payload(),
			);
		}

		compact_statement
	};

	// send the compact version of the statement to any peers which need it.
	circulate_statement(
		ctx,
		relay_parent,
		per_relay_parent,
		per_session,
		&state.candidates,
		&state.authorities,
		&state.peers,
		compact_statement,
	)
	.await;

	if let Some(post_confirmation) = post_confirmation {
		apply_post_confirmation(ctx, state, post_confirmation, reputation).await;
	}

	Ok(())
}

// two kinds of targets: those in our 'cluster' (currently just those in the same group),
// and those we are propagating to through the grid.
#[derive(Debug)]
enum DirectTargetKind {
	Cluster,
	Grid,
}

// Circulates a compact statement to all peers who need it: those in the current group of the
// local validator and grid peers which have already indicated that they know the candidate as
// backed.
//
// We only circulate statements for which we have the confirmed candidate, even to the local group.
//
// The group index which is _canonically assigned_ to this parachain must be
// specified already. This function should not be used when the candidate receipt and
// therefore the canonical group for the parachain is unknown.
//
// preconditions: the candidate entry exists in the state under the relay parent
// and the statement has already been imported into the entry. If this is a `Valid`
// statement, then there must be at least one `Seconded` statement.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn circulate_statement<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	relay_parent_state: &mut PerRelayParentState,
	per_session: &PerSessionState,
	candidates: &Candidates,
	authorities: &HashMap<AuthorityDiscoveryId, PeerId>,
	peers: &HashMap<PeerId, PeerState>,
	statement: SignedStatement,
) {
	let session_info = &per_session.session_info;

	let candidate_hash = *statement.payload().candidate_hash();

	let compact_statement = statement.payload().clone();
	let is_confirmed = candidates.is_confirmed(&candidate_hash);

	let originator = statement.validator_index();
	let (local_validator, targets) = {
		let local_validator = match relay_parent_state.local_validator.as_mut() {
			Some(v) => v,
			None => return, // sanity: nothing to propagate if not a validator.
		};

		let statement_group = per_session.groups.by_validator_index(originator);

		// We're not meant to circulate statements in the cluster until we have the confirmed
		// candidate.
		let cluster_relevant = Some(local_validator.group) == statement_group;
		let cluster_targets = if is_confirmed && cluster_relevant {
			Some(
				local_validator
					.cluster_tracker
					.targets()
					.iter()
					.filter(|&&v| {
						local_validator
							.cluster_tracker
							.can_send(v, originator, compact_statement.clone())
							.is_ok()
					})
					.filter(|&v| v != &local_validator.index)
					.map(|v| (*v, DirectTargetKind::Cluster)),
			)
		} else {
			None
		};

		let grid_targets = local_validator
			.grid_tracker
			.direct_statement_targets(&per_session.groups, originator, &compact_statement)
			.into_iter()
			.filter(|v| !cluster_relevant || !local_validator.cluster_tracker.targets().contains(v))
			.map(|v| (v, DirectTargetKind::Grid));

		let targets = cluster_targets
			.into_iter()
			.flatten()
			.chain(grid_targets)
			.filter_map(|(v, k)| {
				session_info.discovery_keys.get(v.0 as usize).map(|a| (v, a.clone(), k))
			})
			.collect::<Vec<_>>();

		(local_validator, targets)
	};

	let mut statement_to = Vec::new();
	for (target, authority_id, kind) in targets {
		// Find peer ID based on authority ID, and also filter to connected.
		let peer_id: PeerId = match authorities.get(&authority_id) {
			Some(p) if peers.get(p).map_or(false, |p| p.knows_relay_parent(&relay_parent)) => *p,
			None | Some(_) => continue,
		};

		match kind {
			DirectTargetKind::Cluster => {
				// At this point, all peers in the cluster should 'know'
				// the candidate, so we don't expect for this to fail.
				if let Ok(()) = local_validator.cluster_tracker.can_send(
					target,
					originator,
					compact_statement.clone(),
				) {
					local_validator.cluster_tracker.note_sent(
						target,
						originator,
						compact_statement.clone(),
					);
					statement_to.push(peer_id);
				}
			},
			DirectTargetKind::Grid => {
				statement_to.push(peer_id);
				local_validator.grid_tracker.sent_or_received_direct_statement(
					&per_session.groups,
					originator,
					target,
					&compact_statement,
				);
			},
		}
	}

	// ship off the network messages to the network bridge.
	if !statement_to.is_empty() {
		gum::debug!(
			target: LOG_TARGET,
			?compact_statement,
			n_peers = ?statement_to.len(),
			"Sending statement to peers",
		);

		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			statement_to,
			Versioned::VStaging(protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				statement.as_unchecked().clone(),
			))
			.into(),
		))
		.await;
	}
}
/// Check a statement signature under this parent hash.
fn check_statement_signature(
	session_index: SessionIndex,
	validators: &IndexedVec<ValidatorIndex, ValidatorId>,
	relay_parent: Hash,
	statement: UncheckedSignedStatement,
) -> std::result::Result<SignedStatement, UncheckedSignedStatement> {
	let signing_context = SigningContext { session_index, parent_hash: relay_parent };

	validators
		.get(statement.unchecked_validator_index())
		.ok_or_else(|| statement.clone())
		.and_then(|v| statement.try_into_checked(&signing_context, v))
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
	reputation: &mut ReputationAggregator,
	sender: &mut impl overseer::StatementDistributionSenderTrait,
	peer: PeerId,
	rep: Rep,
) {
	reputation.modify(sender, peer, rep).await;
}

/// Handle an incoming statement.
///
/// This checks whether the sender is allowed to send the statement,
/// either via the cluster or the grid.
///
/// This also checks the signature of the statement.
/// If the statement is fresh, this function guarantees that after completion
///   - The statement is re-circulated to all relevant peers in both the cluster and the grid
///   - If the candidate is out-of-cluster and is backable and importable, all statements about the
///     candidate have been sent to backing
///   - If the candidate is in-cluster and is importable, the statement has been sent to backing
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_incoming_statement<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	relay_parent: Hash,
	statement: UncheckedSignedStatement,
	reputation: &mut ReputationAggregator,
) {
	let peer_state = match state.peers.get(&peer) {
		None => {
			// sanity: should be impossible.
			return
		},
		Some(p) => p,
	};

	// Ensure we know the relay parent.
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		None => {
			modify_reputation(
				reputation,
				ctx.sender(),
				peer,
				COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE,
			)
			.await;
			return
		},
		Some(p) => p,
	};

	let per_session = match state.per_session.get(&per_relay_parent.session) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				session = ?per_relay_parent.session,
				"Missing expected session info.",
			);

			return
		},
		Some(s) => s,
	};
	let session_info = &per_session.session_info;

	let local_validator = match per_relay_parent.local_validator.as_mut() {
		None => {
			// we shouldn't be receiving statements unless we're a validator
			// this session.
			modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		},
		Some(l) => l,
	};

	let originator_group =
		match per_session.groups.by_validator_index(statement.unchecked_validator_index()) {
			Some(g) => g,
			None => {
				modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
				return
			},
		};

	let cluster_sender_index = {
		// This block of code only returns `Some` when both the originator and
		// the sending peer are in the cluster.

		let allowed_senders = local_validator
			.cluster_tracker
			.senders_for_originator(statement.unchecked_validator_index());

		allowed_senders
			.iter()
			.filter_map(|i| session_info.discovery_keys.get(i.0 as usize).map(|ad| (*i, ad)))
			.filter(|(_, ad)| peer_state.is_authority(ad))
			.map(|(i, _)| i)
			.next()
	};

	let checked_statement = if let Some(cluster_sender_index) = cluster_sender_index {
		match handle_cluster_statement(
			relay_parent,
			&mut local_validator.cluster_tracker,
			per_relay_parent.session,
			&per_session.session_info,
			statement,
			cluster_sender_index,
		) {
			Ok(Some(s)) => s,
			Ok(None) => return,
			Err(rep) => {
				modify_reputation(reputation, ctx.sender(), peer, rep).await;
				return
			},
		}
	} else {
		let grid_sender_index = local_validator
			.grid_tracker
			.direct_statement_providers(
				&per_session.groups,
				statement.unchecked_validator_index(),
				statement.unchecked_payload(),
			)
			.into_iter()
			.filter_map(|i| session_info.discovery_keys.get(i.0 as usize).map(|ad| (i, ad)))
			.filter(|(_, ad)| peer_state.is_authority(ad))
			.map(|(i, _)| i)
			.next();

		if let Some(grid_sender_index) = grid_sender_index {
			match handle_grid_statement(
				relay_parent,
				&mut local_validator.grid_tracker,
				per_relay_parent.session,
				&per_session,
				statement,
				grid_sender_index,
			) {
				Ok(s) => s,
				Err(rep) => {
					modify_reputation(reputation, ctx.sender(), peer, rep).await;
					return
				},
			}
		} else {
			// Not a cluster or grid peer.
			modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		}
	};

	let statement = checked_statement.payload().clone();
	let originator_index = checked_statement.validator_index();
	let candidate_hash = *checked_statement.payload().candidate_hash();

	// Insert an unconfirmed candidate entry if needed. Note that if the candidate is already
	// confirmed, this ensures that the assigned group of the originator matches the expected group
	// of the parachain.
	{
		let res = state.candidates.insert_unconfirmed(
			peer,
			candidate_hash,
			relay_parent,
			originator_group,
			None,
		);

		if let Err(BadAdvertisement) = res {
			modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		}
	}

	let confirmed = state.candidates.get_confirmed(&candidate_hash);
	let is_confirmed = state.candidates.is_confirmed(&candidate_hash);
	if !is_confirmed {
		// If the candidate is not confirmed, note that we should attempt
		// to request it from the given peer.
		let mut request_entry =
			state
				.request_manager
				.get_or_insert(relay_parent, candidate_hash, originator_group);

		request_entry.add_peer(peer);

		// We only successfully accept statements from the grid on confirmed
		// candidates, therefore this check only passes if the statement is from the cluster
		request_entry.set_cluster_priority();
	}

	let was_fresh = match per_relay_parent.statement_store.insert(
		&per_session.groups,
		checked_statement.clone(),
		StatementOrigin::Remote,
	) {
		Err(statement_store::ValidatorUnknown) => {
			// sanity: should never happen.
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				validator_index = ?originator_index,
				"Error - accepted message from unknown validator."
			);

			return
		},
		Ok(known) => known,
	};

	if was_fresh {
		modify_reputation(reputation, ctx.sender(), peer, BENEFIT_VALID_STATEMENT_FIRST).await;
		let is_importable = state.candidates.is_importable(&candidate_hash);

		if let Some(ref session_topology) = per_session.grid_view {
			local_validator.grid_tracker.learned_fresh_statement(
				&per_session.groups,
				session_topology,
				local_validator.index,
				&statement,
			);
		}

		if let (true, &Some(confirmed)) = (is_importable, &confirmed) {
			send_backing_fresh_statements(
				ctx,
				candidate_hash,
				originator_group,
				&relay_parent,
				&mut *per_relay_parent,
				confirmed,
				per_session,
			)
			.await;
		}

		// We always circulate statements at this point.
		circulate_statement(
			ctx,
			relay_parent,
			per_relay_parent,
			per_session,
			&state.candidates,
			&state.authorities,
			&state.peers,
			checked_statement,
		)
		.await;
	} else {
		modify_reputation(reputation, ctx.sender(), peer, BENEFIT_VALID_STATEMENT).await;
	}
}

/// Checks whether a statement is allowed, whether the signature is accurate,
/// and importing into the cluster tracker if successful.
///
/// if successful, this returns a checked signed statement if it should be imported
/// or otherwise an error indicating a reputational fault.
fn handle_cluster_statement(
	relay_parent: Hash,
	cluster_tracker: &mut ClusterTracker,
	session: SessionIndex,
	session_info: &SessionInfo,
	statement: UncheckedSignedStatement,
	cluster_sender_index: ValidatorIndex,
) -> Result<Option<SignedStatement>, Rep> {
	// additional cluster checks.
	let should_import = {
		match cluster_tracker.can_receive(
			cluster_sender_index,
			statement.unchecked_validator_index(),
			statement.unchecked_payload().clone(),
		) {
			Ok(ClusterAccept::Ok) => true,
			Ok(ClusterAccept::WithPrejudice) => false,
			Err(ClusterRejectIncoming::ExcessiveSeconded) => return Err(COST_EXCESSIVE_SECONDED),
			Err(ClusterRejectIncoming::CandidateUnknown | ClusterRejectIncoming::Duplicate) =>
				return Err(COST_UNEXPECTED_STATEMENT),
			Err(ClusterRejectIncoming::NotInGroup) => {
				// sanity: shouldn't be possible; we already filtered this
				// out above.
				return Err(COST_UNEXPECTED_STATEMENT)
			},
		}
	};

	// Ensure the statement is correctly signed.
	let checked_statement =
		match check_statement_signature(session, &session_info.validators, relay_parent, statement)
		{
			Ok(s) => s,
			Err(_) => return Err(COST_INVALID_SIGNATURE),
		};

	cluster_tracker.note_received(
		cluster_sender_index,
		checked_statement.validator_index(),
		checked_statement.payload().clone(),
	);

	Ok(if should_import { Some(checked_statement) } else { None })
}

/// Checks whether the signature is accurate,
/// importing into the grid tracker if successful.
///
/// if successful, this returns a checked signed statement if it should be imported
/// or otherwise an error indicating a reputational fault.
fn handle_grid_statement(
	relay_parent: Hash,
	grid_tracker: &mut GridTracker,
	session: SessionIndex,
	per_session: &PerSessionState,
	statement: UncheckedSignedStatement,
	grid_sender_index: ValidatorIndex,
) -> Result<SignedStatement, Rep> {
	// Ensure the statement is correctly signed.
	let checked_statement = match check_statement_signature(
		session,
		&per_session.session_info.validators,
		relay_parent,
		statement,
	) {
		Ok(s) => s,
		Err(_) => return Err(COST_INVALID_SIGNATURE),
	};

	grid_tracker.sent_or_received_direct_statement(
		&per_session.groups,
		checked_statement.validator_index(),
		grid_sender_index,
		&checked_statement.payload(),
	);

	Ok(checked_statement)
}

/// Send backing fresh statements. This should only be performed on importable & confirmed
/// candidates.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_backing_fresh_statements<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
	group_index: GroupIndex,
	relay_parent: &Hash,
	relay_parent_state: &mut PerRelayParentState,
	confirmed: &candidates::ConfirmedCandidate,
	per_session: &PerSessionState,
) {
	let group_validators = per_session.groups.get(group_index).unwrap_or(&[]);
	let mut imported = Vec::new();

	for statement in relay_parent_state
		.statement_store
		.fresh_statements_for_backing(group_validators, candidate_hash)
	{
		let v = statement.validator_index();
		let compact = statement.payload().clone();
		imported.push((v, compact));
		let carrying_pvd = statement
			.clone()
			.convert_to_superpayload_with(|statement| match statement {
				CompactStatement::Seconded(_) => FullStatementWithPVD::Seconded(
					(&**confirmed.candidate_receipt()).clone(),
					confirmed.persisted_validation_data().clone(),
				),
				CompactStatement::Valid(c_hash) => FullStatementWithPVD::Valid(c_hash),
			})
			.expect("statements refer to same candidate; qed");

		ctx.send_message(CandidateBackingMessage::Statement(*relay_parent, carrying_pvd))
			.await;
	}

	for (v, s) in imported {
		relay_parent_state.statement_store.note_known_by_backing(v, s);
	}
}

fn local_knowledge_filter(
	group_size: usize,
	group_index: GroupIndex,
	candidate_hash: CandidateHash,
	statement_store: &StatementStore,
) -> StatementFilter {
	let mut f = StatementFilter::blank(group_size);
	statement_store.fill_statement_filter(group_index, candidate_hash, &mut f);
	f
}

// This provides a backable candidate to the grid and dispatches backable candidate announcements
// and acknowledgements via the grid topology. If the session topology is not yet
// available, this will be a no-op.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn provide_candidate_to_grid<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
	relay_parent_state: &mut PerRelayParentState,
	confirmed_candidate: &candidates::ConfirmedCandidate,
	per_session: &PerSessionState,
	authorities: &HashMap<AuthorityDiscoveryId, PeerId>,
	peers: &HashMap<PeerId, PeerState>,
) {
	let local_validator = match relay_parent_state.local_validator {
		Some(ref mut v) => v,
		None => return,
	};

	let relay_parent = confirmed_candidate.relay_parent();
	let group_index = confirmed_candidate.group_index();

	let grid_view = match per_session.grid_view {
		Some(ref t) => t,
		None => {
			gum::debug!(
				target: LOG_TARGET,
				session = relay_parent_state.session,
				"Cannot handle backable candidate due to lack of topology",
			);

			return
		},
	};

	let group_size = match per_session.groups.get(group_index) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				?candidate_hash,
				?relay_parent,
				?group_index,
				session = relay_parent_state.session,
				"Handled backed candidate with unknown group?",
			);

			return
		},
		Some(g) => g.len(),
	};

	let filter = local_knowledge_filter(
		group_size,
		group_index,
		candidate_hash,
		&relay_parent_state.statement_store,
	);

	let actions = local_validator.grid_tracker.add_backed_candidate(
		grid_view,
		candidate_hash,
		group_index,
		filter.clone(),
	);

	let manifest = protocol_vstaging::BackedCandidateManifest {
		relay_parent,
		candidate_hash,
		group_index,
		para_id: confirmed_candidate.para_id(),
		parent_head_data_hash: confirmed_candidate.parent_head_data_hash(),
		statement_knowledge: filter.clone(),
	};
	let acknowledgement = protocol_vstaging::BackedCandidateAcknowledgement {
		candidate_hash,
		statement_knowledge: filter.clone(),
	};

	let manifest_message = Versioned::VStaging(
		protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
	);
	let ack_message = Versioned::VStaging(
		protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(acknowledgement),
	);

	let mut manifest_peers = Vec::new();
	let mut ack_peers = Vec::new();

	let mut post_statements = Vec::new();
	for (v, action) in actions {
		let p = match connected_validator_peer(authorities, per_session, v) {
			None => continue,
			Some(p) =>
				if peers.get(&p).map_or(false, |d| d.knows_relay_parent(&relay_parent)) {
					p
				} else {
					continue
				},
		};

		match action {
			grid::ManifestKind::Full => manifest_peers.push(p),
			grid::ManifestKind::Acknowledgement => ack_peers.push(p),
		}

		local_validator.grid_tracker.manifest_sent_to(
			&per_session.groups,
			v,
			candidate_hash,
			filter.clone(),
		);
		post_statements.extend(
			post_acknowledgement_statement_messages(
				v,
				relay_parent,
				&mut local_validator.grid_tracker,
				&relay_parent_state.statement_store,
				&per_session.groups,
				group_index,
				candidate_hash,
			)
			.into_iter()
			.map(|m| (vec![p], m)),
		);
	}

	if !manifest_peers.is_empty() {
		gum::debug!(
			target: LOG_TARGET,
			?candidate_hash,
			n_peers = manifest_peers.len(),
			"Sending manifest to peers"
		);

		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			manifest_peers,
			manifest_message.into(),
		))
		.await;
	}

	if !ack_peers.is_empty() {
		gum::debug!(
			target: LOG_TARGET,
			?candidate_hash,
			n_peers = ack_peers.len(),
			"Sending acknowledgement to peers"
		);

		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			ack_peers,
			ack_message.into(),
		))
		.await;
	}

	if !post_statements.is_empty() {
		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessages(post_statements))
			.await;
	}
}

fn group_for_para(
	availability_cores: &[CoreState],
	group_rotation_info: &GroupRotationInfo,
	para_id: ParaId,
) -> Option<GroupIndex> {
	// Note: this won't work well for on-demand parachains as it assumes that core assignments are
	// fixed across blocks.
	let core_index = availability_cores.iter().position(|c| c.para_id() == Some(para_id));

	core_index
		.map(|c| group_rotation_info.group_for_core(CoreIndex(c as _), availability_cores.len()))
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn fragment_tree_update_inner<Context>(
	ctx: &mut Context,
	state: &mut State,
	active_leaf_hash: Option<Hash>,
	required_parent_info: Option<(Hash, ParaId)>,
	known_hypotheticals: Option<Vec<HypotheticalCandidate>>,
) {
	// 1. get hypothetical candidates
	let hypotheticals = match known_hypotheticals {
		None => state.candidates.frontier_hypotheticals(required_parent_info),
		Some(h) => h,
	};

	// 2. find out which are in the frontier
	let frontier = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(ProspectiveParachainsMessage::GetHypotheticalFrontier(
			HypotheticalFrontierRequest {
				candidates: hypotheticals,
				fragment_tree_relay_parent: active_leaf_hash,
				backed_in_path_only: false,
			},
			tx,
		))
		.await;

		match rx.await {
			Ok(frontier) => frontier,
			Err(oneshot::Canceled) => return,
		}
	};
	// 3. note that they are importable under a given leaf hash.
	for (hypo, membership) in frontier {
		// skip parablocks outside of the frontier
		if membership.is_empty() {
			continue
		}

		for (leaf_hash, _) in membership {
			state.candidates.note_importable_under(&hypo, leaf_hash);
		}

		// 4. for confirmed candidates, send all statements which are new to backing.
		if let HypotheticalCandidate::Complete {
			candidate_hash,
			receipt,
			persisted_validation_data: _,
		} = hypo
		{
			let confirmed_candidate = state.candidates.get_confirmed(&candidate_hash);
			let prs = state.per_relay_parent.get_mut(&receipt.descriptor().relay_parent);
			if let (Some(confirmed), Some(prs)) = (confirmed_candidate, prs) {
				let group_index = group_for_para(
					&prs.availability_cores,
					&prs.group_rotation_info,
					receipt.descriptor().para_id,
				);

				let per_session = state.per_session.get(&prs.session);
				if let (Some(per_session), Some(group_index)) = (per_session, group_index) {
					send_backing_fresh_statements(
						ctx,
						candidate_hash,
						group_index,
						&receipt.descriptor().relay_parent,
						prs,
						confirmed,
						per_session,
					)
					.await;
				}
			}
		}
	}
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn new_leaf_fragment_tree_updates<Context>(
	ctx: &mut Context,
	state: &mut State,
	leaf_hash: Hash,
) {
	fragment_tree_update_inner(ctx, state, Some(leaf_hash), None, None).await
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn prospective_backed_notification_fragment_tree_updates<Context>(
	ctx: &mut Context,
	state: &mut State,
	para_id: ParaId,
	para_head: Hash,
) {
	fragment_tree_update_inner(ctx, state, None, Some((para_head, para_id)), None).await
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn new_confirmed_candidate_fragment_tree_updates<Context>(
	ctx: &mut Context,
	state: &mut State,
	candidate: HypotheticalCandidate,
) {
	fragment_tree_update_inner(ctx, state, None, None, Some(vec![candidate])).await
}

struct ManifestImportSuccess<'a> {
	relay_parent_state: &'a mut PerRelayParentState,
	per_session: &'a PerSessionState,
	acknowledge: bool,
	sender_index: ValidatorIndex,
}

/// Handles the common part of incoming manifests of both types (full & acknowledgement)
///
/// Basic sanity checks around data, importing the manifest into the grid tracker, finding the
/// sending peer's validator index, reporting the peer for any misbehavior, etc.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_incoming_manifest_common<'a, Context>(
	ctx: &mut Context,
	peer: PeerId,
	peers: &HashMap<PeerId, PeerState>,
	per_relay_parent: &'a mut HashMap<Hash, PerRelayParentState>,
	per_session: &'a HashMap<SessionIndex, PerSessionState>,
	candidates: &mut Candidates,
	candidate_hash: CandidateHash,
	relay_parent: Hash,
	para_id: ParaId,
	manifest_summary: grid::ManifestSummary,
	manifest_kind: grid::ManifestKind,
	reputation: &mut ReputationAggregator,
) -> Option<ManifestImportSuccess<'a>> {
	// 1. sanity checks: peer is connected, relay-parent in state, para ID matches group index.
	let peer_state = match peers.get(&peer) {
		None => return None,
		Some(p) => p,
	};

	let relay_parent_state = match per_relay_parent.get_mut(&relay_parent) {
		None => {
			modify_reputation(
				reputation,
				ctx.sender(),
				peer,
				COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE,
			)
			.await;
			return None
		},
		Some(s) => s,
	};

	let per_session = match per_session.get(&relay_parent_state.session) {
		None => return None,
		Some(s) => s,
	};

	let local_validator = match relay_parent_state.local_validator.as_mut() {
		None => {
			modify_reputation(
				reputation,
				ctx.sender(),
				peer,
				COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE,
			)
			.await;
			return None
		},
		Some(x) => x,
	};

	let expected_group = group_for_para(
		&relay_parent_state.availability_cores,
		&relay_parent_state.group_rotation_info,
		para_id,
	);

	if expected_group != Some(manifest_summary.claimed_group_index) {
		modify_reputation(reputation, ctx.sender(), peer, COST_MALFORMED_MANIFEST).await;
		return None
	}

	let grid_topology = match per_session.grid_view.as_ref() {
		None => return None,
		Some(x) => x,
	};

	let sender_index = grid_topology
		.iter_sending_for_group(manifest_summary.claimed_group_index, manifest_kind)
		.filter_map(|i| per_session.session_info.discovery_keys.get(i.0 as usize).map(|ad| (i, ad)))
		.filter(|(_, ad)| peer_state.is_authority(ad))
		.map(|(i, _)| i)
		.next();

	let sender_index = match sender_index {
		None => {
			modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_MANIFEST_DISALLOWED)
				.await;
			return None
		},
		Some(s) => s,
	};

	// 2. sanity checks: peer is validator, bitvec size, import into grid tracker
	let group_index = manifest_summary.claimed_group_index;
	let claimed_parent_hash = manifest_summary.claimed_parent_hash;
	let acknowledge = match local_validator.grid_tracker.import_manifest(
		grid_topology,
		&per_session.groups,
		candidate_hash,
		relay_parent_state.seconding_limit,
		manifest_summary,
		manifest_kind,
		sender_index,
	) {
		Ok(x) => x,
		Err(grid::ManifestImportError::Conflicting) => {
			modify_reputation(reputation, ctx.sender(), peer, COST_CONFLICTING_MANIFEST).await;
			return None
		},
		Err(grid::ManifestImportError::Overflow) => {
			modify_reputation(reputation, ctx.sender(), peer, COST_EXCESSIVE_SECONDED).await;
			return None
		},
		Err(grid::ManifestImportError::Insufficient) => {
			modify_reputation(reputation, ctx.sender(), peer, COST_INSUFFICIENT_MANIFEST).await;
			return None
		},
		Err(grid::ManifestImportError::Malformed) => {
			modify_reputation(reputation, ctx.sender(), peer, COST_MALFORMED_MANIFEST).await;
			return None
		},
		Err(grid::ManifestImportError::Disallowed) => {
			modify_reputation(reputation, ctx.sender(), peer, COST_UNEXPECTED_MANIFEST_DISALLOWED)
				.await;
			return None
		},
	};

	// 3. if accepted by grid, insert as unconfirmed.
	if let Err(BadAdvertisement) = candidates.insert_unconfirmed(
		peer,
		candidate_hash,
		relay_parent,
		group_index,
		Some((claimed_parent_hash, para_id)),
	) {
		modify_reputation(reputation, ctx.sender(), peer, COST_INACCURATE_ADVERTISEMENT).await;
		return None
	}

	Some(ManifestImportSuccess { relay_parent_state, per_session, acknowledge, sender_index })
}

/// Produce a list of network messages to send to a peer, following acknowledgement of a manifest.
/// This notes the messages as sent within the grid state.
fn post_acknowledgement_statement_messages(
	recipient: ValidatorIndex,
	relay_parent: Hash,
	grid_tracker: &mut GridTracker,
	statement_store: &StatementStore,
	groups: &Groups,
	group_index: GroupIndex,
	candidate_hash: CandidateHash,
) -> Vec<net_protocol::VersionedValidationProtocol> {
	let sending_filter = match grid_tracker.pending_statements_for(recipient, candidate_hash) {
		None => return Vec::new(),
		Some(f) => f,
	};

	let mut messages = Vec::new();
	for statement in
		statement_store.group_statements(groups, group_index, candidate_hash, &sending_filter)
	{
		grid_tracker.sent_or_received_direct_statement(
			groups,
			statement.validator_index(),
			recipient,
			statement.payload(),
		);

		messages.push(Versioned::VStaging(
			protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				statement.as_unchecked().clone(),
			)
			.into(),
		));
	}

	messages
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_incoming_manifest<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	manifest: net_protocol::vstaging::BackedCandidateManifest,
	reputation: &mut ReputationAggregator,
) {
	gum::debug!(
		target: LOG_TARGET,
		candidate_hash = ?manifest.candidate_hash,
		?peer,
		"Received incoming manifest",
	);

	let x = match handle_incoming_manifest_common(
		ctx,
		peer,
		&state.peers,
		&mut state.per_relay_parent,
		&state.per_session,
		&mut state.candidates,
		manifest.candidate_hash,
		manifest.relay_parent,
		manifest.para_id,
		grid::ManifestSummary {
			claimed_parent_hash: manifest.parent_head_data_hash,
			claimed_group_index: manifest.group_index,
			statement_knowledge: manifest.statement_knowledge,
		},
		grid::ManifestKind::Full,
		reputation,
	)
	.await
	{
		Some(x) => x,
		None => return,
	};

	let ManifestImportSuccess { relay_parent_state, per_session, acknowledge, sender_index } = x;

	if acknowledge {
		// 4. if already known within grid (confirmed & backed), acknowledge candidate
		gum::trace!(
			target: LOG_TARGET,
			candidate_hash = ?manifest.candidate_hash,
			"Known candidate - acknowledging manifest",
		);

		let local_knowledge = {
			let group_size = match per_session.groups.get(manifest.group_index) {
				None => return, // sanity
				Some(x) => x.len(),
			};

			local_knowledge_filter(
				group_size,
				manifest.group_index,
				manifest.candidate_hash,
				&relay_parent_state.statement_store,
			)
		};

		let messages = acknowledgement_and_statement_messages(
			peer,
			sender_index,
			&per_session.groups,
			relay_parent_state,
			manifest.relay_parent,
			manifest.group_index,
			manifest.candidate_hash,
			local_knowledge,
		);

		if !messages.is_empty() {
			ctx.send_message(NetworkBridgeTxMessage::SendValidationMessages(messages)).await;
		}
	} else if !state.candidates.is_confirmed(&manifest.candidate_hash) {
		// 5. if unconfirmed, add request entry
		gum::trace!(
			target: LOG_TARGET,
			candidate_hash = ?manifest.candidate_hash,
			"Unknown candidate - requesting",
		);

		state
			.request_manager
			.get_or_insert(manifest.relay_parent, manifest.candidate_hash, manifest.group_index)
			.add_peer(peer);
	}
}

/// Produces acknowledgement and statement messages to be sent over the network,
/// noting that they have been sent within the grid topology tracker as well.
fn acknowledgement_and_statement_messages(
	peer: PeerId,
	validator_index: ValidatorIndex,
	groups: &Groups,
	relay_parent_state: &mut PerRelayParentState,
	relay_parent: Hash,
	group_index: GroupIndex,
	candidate_hash: CandidateHash,
	local_knowledge: StatementFilter,
) -> Vec<(Vec<PeerId>, net_protocol::VersionedValidationProtocol)> {
	let local_validator = match relay_parent_state.local_validator.as_mut() {
		None => return Vec::new(),
		Some(l) => l,
	};

	let acknowledgement = protocol_vstaging::BackedCandidateAcknowledgement {
		candidate_hash,
		statement_knowledge: local_knowledge.clone(),
	};

	let msg = Versioned::VStaging(
		protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(acknowledgement),
	);

	let mut messages = vec![(vec![peer], msg.into())];

	local_validator.grid_tracker.manifest_sent_to(
		groups,
		validator_index,
		candidate_hash,
		local_knowledge.clone(),
	);

	let statement_messages = post_acknowledgement_statement_messages(
		validator_index,
		relay_parent,
		&mut local_validator.grid_tracker,
		&relay_parent_state.statement_store,
		&groups,
		group_index,
		candidate_hash,
	);

	messages.extend(statement_messages.into_iter().map(|m| (vec![peer], m)));

	messages
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_incoming_acknowledgement<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	acknowledgement: net_protocol::vstaging::BackedCandidateAcknowledgement,
	reputation: &mut ReputationAggregator,
) {
	// The key difference between acknowledgments and full manifests is that only
	// the candidate hash is included alongside the bitfields, so the candidate
	// must be confirmed for us to even process it.

	gum::debug!(
		target: LOG_TARGET,
		candidate_hash = ?acknowledgement.candidate_hash,
		?peer,
		"Received incoming acknowledgement",
	);

	let candidate_hash = acknowledgement.candidate_hash;
	let (relay_parent, parent_head_data_hash, group_index, para_id) = {
		match state.candidates.get_confirmed(&candidate_hash) {
			Some(c) => (c.relay_parent(), c.parent_head_data_hash(), c.group_index(), c.para_id()),
			None => {
				modify_reputation(
					reputation,
					ctx.sender(),
					peer,
					COST_UNEXPECTED_ACKNOWLEDGEMENT_UNKNOWN_CANDIDATE,
				)
				.await;
				return
			},
		}
	};

	let x = match handle_incoming_manifest_common(
		ctx,
		peer,
		&state.peers,
		&mut state.per_relay_parent,
		&state.per_session,
		&mut state.candidates,
		candidate_hash,
		relay_parent,
		para_id,
		grid::ManifestSummary {
			claimed_parent_hash: parent_head_data_hash,
			claimed_group_index: group_index,
			statement_knowledge: acknowledgement.statement_knowledge,
		},
		grid::ManifestKind::Acknowledgement,
		reputation,
	)
	.await
	{
		Some(x) => x,
		None => return,
	};

	let ManifestImportSuccess { relay_parent_state, per_session, sender_index, .. } = x;

	let local_validator = match relay_parent_state.local_validator.as_mut() {
		None => return,
		Some(l) => l,
	};

	let messages = post_acknowledgement_statement_messages(
		sender_index,
		relay_parent,
		&mut local_validator.grid_tracker,
		&relay_parent_state.statement_store,
		&per_session.groups,
		group_index,
		candidate_hash,
	);

	if !messages.is_empty() {
		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessages(
			messages.into_iter().map(|m| (vec![peer], m)).collect(),
		))
		.await;
	}
}

/// Handle a notification of a candidate being backed.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_backed_candidate_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	candidate_hash: CandidateHash,
) {
	// If the candidate is unknown or unconfirmed, it's a race (pruned before receiving message)
	// or a bug. Ignore if so
	let confirmed = match state.candidates.get_confirmed(&candidate_hash) {
		None => {
			gum::debug!(
				target: LOG_TARGET,
				?candidate_hash,
				"Received backed candidate notification for unknown or unconfirmed",
			);

			return
		},
		Some(c) => c,
	};

	let relay_parent_state = match state.per_relay_parent.get_mut(&confirmed.relay_parent()) {
		None => return,
		Some(s) => s,
	};

	let per_session = match state.per_session.get(&relay_parent_state.session) {
		None => return,
		Some(s) => s,
	};

	gum::debug!(
		target: LOG_TARGET,
		?candidate_hash,
		group_index = ?confirmed.group_index(),
		"Candidate Backed - initiating grid distribution & child fetches"
	);

	provide_candidate_to_grid(
		ctx,
		candidate_hash,
		relay_parent_state,
		confirmed,
		per_session,
		&state.authorities,
		&state.peers,
	)
	.await;

	// Search for children of the backed candidate to request.
	prospective_backed_notification_fragment_tree_updates(
		ctx,
		state,
		confirmed.para_id(),
		confirmed.candidate_receipt().descriptor().para_head,
	)
	.await;
}

/// Sends all messages about a candidate to all peers in the cluster,
/// with `Seconded` statements first.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_cluster_candidate_statements<Context>(
	ctx: &mut Context,
	state: &mut State,
	candidate_hash: CandidateHash,
	relay_parent: Hash,
) {
	let relay_parent_state = match state.per_relay_parent.get_mut(&relay_parent) {
		None => return,
		Some(s) => s,
	};

	let per_session = match state.per_session.get(&relay_parent_state.session) {
		None => return,
		Some(s) => s,
	};

	let local_group = match relay_parent_state.local_validator.as_mut() {
		None => return,
		Some(v) => v.group,
	};

	let group_size = match per_session.groups.get(local_group) {
		None => return,
		Some(g) => g.len(),
	};

	let statements: Vec<_> = relay_parent_state
		.statement_store
		.group_statements(
			&per_session.groups,
			local_group,
			candidate_hash,
			&StatementFilter::full(group_size),
		)
		.map(|x| x.clone())
		.collect();

	for statement in statements {
		circulate_statement(
			ctx,
			relay_parent,
			relay_parent_state,
			per_session,
			&state.candidates,
			&state.authorities,
			&state.peers,
			statement,
		)
		.await;
	}
}

/// Applies state & p2p updates as a result of a newly confirmed candidate.
///
/// This punishes peers which advertised the candidate incorrectly, as well as
/// doing an importability analysis of the confirmed candidate and providing
/// statements to the backing subsystem if importable. It also cleans up
/// any pending requests for the candidate.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn apply_post_confirmation<Context>(
	ctx: &mut Context,
	state: &mut State,
	post_confirmation: PostConfirmation,
	reputation: &mut ReputationAggregator,
) {
	for peer in post_confirmation.reckoning.incorrect {
		modify_reputation(reputation, ctx.sender(), peer, COST_INACCURATE_ADVERTISEMENT).await;
	}

	let candidate_hash = post_confirmation.hypothetical.candidate_hash();
	state.request_manager.remove_for(candidate_hash);

	send_cluster_candidate_statements(
		ctx,
		state,
		candidate_hash,
		post_confirmation.hypothetical.relay_parent(),
	)
	.await;
	new_confirmed_candidate_fragment_tree_updates(ctx, state, post_confirmation.hypothetical).await;
}

/// Dispatch pending requests for candidate data & statements.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn dispatch_requests<Context>(ctx: &mut Context, state: &mut State) {
	if !state.request_manager.has_pending_requests() {
		return
	}

	let peers = &state.peers;
	let peer_advertised = |identifier: &CandidateIdentifier, peer: &_| {
		let peer_data = peers.get(peer)?;

		let relay_parent_state = state.per_relay_parent.get(&identifier.relay_parent)?;
		let per_session = state.per_session.get(&relay_parent_state.session)?;

		let local_validator = relay_parent_state.local_validator.as_ref()?;

		for validator_id in find_validator_ids(peer_data.iter_known_discovery_ids(), |a| {
			per_session.authority_lookup.get(a)
		}) {
			// For cluster members, they haven't advertised any statements in particular,
			// but have surely sent us some.
			if local_validator
				.cluster_tracker
				.knows_candidate(validator_id, identifier.candidate_hash)
			{
				return Some(StatementFilter::blank(local_validator.cluster_tracker.targets().len()))
			}

			let filter = local_validator
				.grid_tracker
				.advertised_statements(validator_id, &identifier.candidate_hash);

			if let Some(f) = filter {
				return Some(f)
			}
		}

		None
	};
	let request_props = |identifier: &CandidateIdentifier| {
		let &CandidateIdentifier { relay_parent, group_index, .. } = identifier;

		let relay_parent_state = state.per_relay_parent.get(&relay_parent)?;
		let per_session = state.per_session.get(&relay_parent_state.session)?;
		let group = per_session.groups.get(group_index)?;
		let seconding_limit = relay_parent_state.seconding_limit;

		// Request nothing which would be an 'over-seconded' statement.
		let mut unwanted_mask = StatementFilter::blank(group.len());
		for (i, v) in group.iter().enumerate() {
			if relay_parent_state.statement_store.seconded_count(v) >= seconding_limit {
				unwanted_mask.seconded_in_group.set(i, true);
			}
		}

		// don't require a backing threshold for cluster candidates.
		let require_backing = relay_parent_state.local_validator.as_ref()?.group != group_index;

		Some(RequestProperties {
			unwanted_mask,
			backing_threshold: if require_backing {
				Some(polkadot_node_primitives::minimum_votes(group.len()))
			} else {
				None
			},
		})
	};

	while let Some(request) = state.request_manager.next_request(
		&mut state.response_manager,
		request_props,
		peer_advertised,
	) {
		// Peer is supposedly connected.
		ctx.send_message(NetworkBridgeTxMessage::SendRequests(
			vec![Requests::AttestedCandidateVStaging(request)],
			IfDisconnected::ImmediateError,
		))
		.await;
	}
}

/// Wait on the next incoming response. If there are no requests pending, this
/// future never resolves. It is the responsibility of the user of this API
/// to interrupt the future.
pub(crate) async fn receive_response(response_manager: &mut ResponseManager) -> UnhandledResponse {
	match response_manager.incoming().await {
		Some(r) => r,
		None => futures::future::pending().await,
	}
}

/// Wait on the next soonest retry on a pending request. If there are no retries pending, this
/// future never resolves. Note that this only signals that a request is ready to retry; the user of
/// this API must call `dispatch_requests`.
pub(crate) async fn next_retry(request_manager: &mut RequestManager) {
	match request_manager.next_retry_time() {
		Some(instant) =>
			futures_timer::Delay::new(instant.saturating_duration_since(Instant::now())).await,
		None => futures::future::pending().await,
	}
}

/// Handles an incoming response. This does the actual work of validating the response,
/// importing statements, sending acknowledgements, etc.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_response<Context>(
	ctx: &mut Context,
	state: &mut State,
	response: UnhandledResponse,
	reputation: &mut ReputationAggregator,
) {
	let &requests::CandidateIdentifier { relay_parent, candidate_hash, group_index } =
		response.candidate_identifier();

	let post_confirmation = {
		let relay_parent_state = match state.per_relay_parent.get_mut(&relay_parent) {
			None => return,
			Some(s) => s,
		};

		let per_session = match state.per_session.get(&relay_parent_state.session) {
			None => return,
			Some(s) => s,
		};

		let group = match per_session.groups.get(group_index) {
			None => return,
			Some(g) => g,
		};

		let res = response.validate_response(
			&mut state.request_manager,
			group,
			relay_parent_state.session,
			|v| per_session.session_info.validators.get(v).map(|x| x.clone()),
			|para, g_index| {
				let expected_group = group_for_para(
					&relay_parent_state.availability_cores,
					&relay_parent_state.group_rotation_info,
					para,
				);

				Some(g_index) == expected_group
			},
		);

		for (peer, rep) in res.reputation_changes {
			modify_reputation(reputation, ctx.sender(), peer, rep).await;
		}

		let (candidate, pvd, statements) = match res.request_status {
			requests::CandidateRequestStatus::Outdated => return,
			requests::CandidateRequestStatus::Incomplete => return,
			requests::CandidateRequestStatus::Complete {
				candidate,
				persisted_validation_data,
				statements,
			} => (candidate, persisted_validation_data, statements),
		};

		for statement in statements {
			let _ = relay_parent_state.statement_store.insert(
				&per_session.groups,
				statement,
				StatementOrigin::Remote,
			);
		}

		if let Some(post_confirmation) =
			state.candidates.confirm_candidate(candidate_hash, candidate, pvd, group_index)
		{
			post_confirmation
		} else {
			gum::warn!(
				target: LOG_TARGET,
				?candidate_hash,
				"Candidate re-confirmed by request/response: logic error",
			);

			return
		}
	};

	// Note that this implicitly circulates all statements via the cluster.
	apply_post_confirmation(ctx, state, post_confirmation, reputation).await;

	let confirmed = state.candidates.get_confirmed(&candidate_hash).expect("just confirmed; qed");

	// Although the candidate is confirmed, it isn't yet on the
	// hypothetical frontier of the fragment tree. Later, when it is,
	// we will import statements.
	if !confirmed.is_importable(None) {
		return
	}

	let relay_parent_state = match state.per_relay_parent.get_mut(&relay_parent) {
		None => return,
		Some(s) => s,
	};

	let per_session = match state.per_session.get(&relay_parent_state.session) {
		None => return,
		Some(s) => s,
	};

	send_backing_fresh_statements(
		ctx,
		candidate_hash,
		group_index,
		&relay_parent,
		relay_parent_state,
		confirmed,
		per_session,
	)
	.await;

	// we don't need to send acknowledgement yet because
	// 1. the candidate is not known yet, so cannot be backed. any previous confirmation is a bug,
	//    because `apply_post_confirmation` is meant to clear requests.
	// 2. providing the statements to backing will lead to 'Backed' message.
	// 3. on 'Backed' we will send acknowledgements/follow up statements when this becomes
	//    includable.
}

/// Answer an incoming request for a candidate.
pub(crate) fn answer_request(state: &mut State, message: ResponderMessage) {
	let ResponderMessage { request, sent_feedback } = message;
	let AttestedCandidateRequest { candidate_hash, ref mask } = &request.payload;

	// Signal to the responder that we started processing this request.
	let _ = sent_feedback.send(());

	let confirmed = match state.candidates.get_confirmed(&candidate_hash) {
		None => return, // drop request, candidate not known.
		Some(c) => c,
	};

	let relay_parent_state = match state.per_relay_parent.get(&confirmed.relay_parent()) {
		None => return,
		Some(s) => s,
	};

	let local_validator = match relay_parent_state.local_validator.as_ref() {
		None => return,
		Some(s) => s,
	};

	let per_session = match state.per_session.get(&relay_parent_state.session) {
		None => return,
		Some(s) => s,
	};

	let peer_data = match state.peers.get(&request.peer) {
		None => return,
		Some(d) => d,
	};

	let group_size = per_session
		.groups
		.get(confirmed.group_index())
		.expect("group from session's candidate always known; qed")
		.len();

	// check request bitfields are right size.
	if mask.seconded_in_group.len() != group_size || mask.validated_in_group.len() != group_size {
		let _ = request.send_outgoing_response(OutgoingResponse {
			result: Err(()),
			reputation_changes: vec![COST_INVALID_REQUEST_BITFIELD_SIZE],
			sent_feedback: None,
		});

		return
	}

	// check peer is allowed to request the candidate (i.e. we've sent them a manifest)
	{
		let mut can_request = false;
		for validator_id in find_validator_ids(peer_data.iter_known_discovery_ids(), |a| {
			per_session.authority_lookup.get(a)
		}) {
			if local_validator.grid_tracker.can_request(validator_id, *candidate_hash) {
				can_request = true;
				break
			}
		}

		if !can_request {
			let _ = request.send_outgoing_response(OutgoingResponse {
				result: Err(()),
				reputation_changes: vec![COST_UNEXPECTED_REQUEST],
				sent_feedback: None,
			});

			return
		}
	}

	// Transform mask with 'OR' semantics into one with 'AND' semantics for the API used
	// below.
	let and_mask = StatementFilter {
		seconded_in_group: !mask.seconded_in_group.clone(),
		validated_in_group: !mask.validated_in_group.clone(),
	};

	let response = AttestedCandidateResponse {
		candidate_receipt: (&**confirmed.candidate_receipt()).clone(),
		persisted_validation_data: confirmed.persisted_validation_data().clone(),
		statements: relay_parent_state
			.statement_store
			.group_statements(
				&per_session.groups,
				confirmed.group_index(),
				*candidate_hash,
				&and_mask,
			)
			.map(|s| s.as_unchecked().clone())
			.collect(),
	};

	let _ = request.send_response(response);
}

/// Messages coming from the background respond task.
pub(crate) struct ResponderMessage {
	request: IncomingRequest<AttestedCandidateRequest>,
	sent_feedback: oneshot::Sender<()>,
}

/// A fetching task, taking care of fetching candidates via request/response.
///
/// Runs in a background task and feeds request to [`answer_request`] through [`MuxedMessage`].
pub(crate) async fn respond_task(
	mut receiver: IncomingRequestReceiver<AttestedCandidateRequest>,
	mut sender: mpsc::Sender<ResponderMessage>,
) {
	let mut pending_out = FuturesUnordered::new();
	loop {
		// Ensure we are not handling too many requests in parallel.
		if pending_out.len() >= MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS as usize {
			// Wait for one to finish:
			pending_out.next().await;
		}

		let req = match receiver.recv(|| vec![COST_INVALID_REQUEST]).await.into_nested() {
			Ok(Ok(v)) => v,
			Err(fatal) => {
				gum::debug!(target: LOG_TARGET, error = ?fatal, "Shutting down request responder");
				return
			},
			Ok(Err(jfyi)) => {
				gum::debug!(target: LOG_TARGET, error = ?jfyi, "Decoding request failed");
				continue
			},
		};

		let (pending_sent_tx, pending_sent_rx) = oneshot::channel();
		if let Err(err) = sender
			.feed(ResponderMessage { request: req, sent_feedback: pending_sent_tx })
			.await
		{
			gum::debug!(target: LOG_TARGET, ?err, "Shutting down responder");
			return
		}
		pending_out.push(pending_sent_rx);
	}
}
