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
	grid_topology::{RequiredRouting, SessionGridTopology},
	peer_set::ValidationVersion,
	vstaging as protocol_vstaging, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{
	SignedFullStatementWithPVD, StatementWithPVD as FullStatementWithPVD,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{CandidateBackingMessage, NetworkBridgeEvent, NetworkBridgeTxMessage},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, PerLeafSpan, StatementDistributionSenderTrait,
};
use polkadot_node_subsystem_util::backing_implicit_view::{FetchError, View as ImplicitView};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, CommittedCandidateReceipt, CompactStatement, CoreState,
	GroupIndex, Hash, Id as ParaId, PersistedValidationData, SessionIndex, SessionInfo,
	SignedStatement, SigningContext, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
};

use sp_keystore::SyncCryptoStorePtr;

use indexmap::IndexMap;

use std::collections::{
	hash_map::{Entry, HashMap},
	HashSet,
};

use crate::{
	error::{JfyiError, JfyiErrorResult},
	LOG_TARGET,
};
use candidates::{BadAdvertisement, Candidates};
use cluster::{Accept as ClusterAccept, ClusterTracker, RejectIncoming as ClusterRejectIncoming};
use grid::{GridTracker, ManifestSummary, StatementFilter};
use groups::Groups;
use requests::RequestManager;
use statement_store::StatementStore;

mod candidates;
mod cluster;
mod grid;
mod groups;
mod requests;
mod statement_store;

const COST_UNEXPECTED_STATEMENT: Rep = Rep::CostMinor("Unexpected Statement");
const COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE: Rep =
	Rep::CostMinor("Unexpected Statement, missing knowlege for relay parent");
const COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE: Rep =
	Rep::CostMinor("Unexpected Statement, unknown candidate");
const COST_UNEXPECTED_STATEMENT_REMOTE: Rep =
	Rep::CostMinor("Unexpected Statement, remote not allowed");
const COST_EXCESSIVE_SECONDED: Rep = Rep::CostMinor("Sent Excessive `Seconded` Statements");

const COST_INVALID_SIGNATURE: Rep = Rep::CostMajor("Invalid Statement Signature");
const COST_IMPROPERLY_DECODED_RESPONSE: Rep =
	Rep::CostMajor("Improperly Encoded Candidate Response");
const COST_INVALID_RESPONSE: Rep = Rep::CostMajor("Invalid Candidate Response");
const COST_UNREQUESTED_RESPONSE_STATEMENT: Rep =
	Rep::CostMajor("Un-requested Statement In Response");
const COST_INACCURATE_ADVERTISEMENT: Rep =
	Rep::CostMajor("Peer advertised a candidate inaccurately");

const BENEFIT_VALID_RESPONSE: Rep = Rep::BenefitMajor("Peer Answered Candidate Request");
const BENEFIT_VALID_STATEMENT: Rep = Rep::BenefitMajor("Peer provided a valid statement");
const BENEFIT_VALID_STATEMENT_FIRST: Rep =
	Rep::BenefitMajorFirst("Peer was the first to provide a valid statement");

struct PerRelayParentState {
	validator_state: HashMap<ValidatorIndex, PerRelayParentValidatorState>,
	local_validator: Option<LocalValidatorState>,
	statement_store: StatementStore,
	session: SessionIndex,
}

struct PerRelayParentValidatorState {
	seconded_count: usize,
	group_id: GroupIndex,
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
	async fn new(session_info: SessionInfo, keystore: &SyncCryptoStorePtr) -> Self {
		let groups =
			Groups::new(session_info.validator_groups.clone(), &session_info.discovery_keys);
		let mut authority_lookup = HashMap::new();
		for (i, ad) in session_info.discovery_keys.iter().cloned().enumerate() {
			authority_lookup.insert(ad, ValidatorIndex(i as _));
		}

		let local_validator =
			polkadot_node_subsystem_util::signing_key_and_index(&session_info.validators, keystore)
				.await;

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
			&self.session_info.validator_groups[..],
			topology,
			self.local_validator,
		);

		self.grid_view = Some(grid_view);
	}

	fn authority_index_in_session(
		&self,
		discovery_key: &AuthorityDiscoveryId,
	) -> Option<ValidatorIndex> {
		self.authority_lookup.get(discovery_key).map(|x| *x)
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
	keystore: SyncCryptoStorePtr,
	authorities: HashMap<AuthorityDiscoveryId, PeerId>,
	request_manager: RequestManager,
}

// For the provided validator index, if there is a connected peer
// controlling the given authority ID,
// TODO [now]: finish above doc.
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
		.map(|p| p.clone())
}

struct PeerState {
	view: View,
	implicit_view: HashSet<Hash>,
	discovery_ids: Option<HashSet<AuthorityDiscoveryId>>,
}

impl PeerState {
	// Whether we know that a peer knows a relay-parent.
	// The peer knows the relay-parent if it is either implicit or explicit
	// in their view. However, if it is implicit via an active-leaf we don't
	// recognize, we will not accurately be able to recognize them as 'knowing'
	// the relay-parent.
	fn knows_relay_parent(&self, relay_parent: &Hash) -> bool {
		self.implicit_view.contains(relay_parent)
	}

	fn is_authority(&self, authority_id: &AuthorityDiscoveryId) -> bool {
		self.discovery_ids.as_ref().map_or(false, |x| x.contains(authority_id))
	}
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_network_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: NetworkBridgeEvent<net_protocol::StatementDistributionMessage>,
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
				for discovery_key in p.discovery_ids.into_iter().flat_map(|x| x) {
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

			// TODO [now] for all relay-parents with this session, send all grid peers
			// any `BackedCandidateInv` messages they might need.
		},
		NetworkBridgeEvent::PeerMessage(peer_id, message) => {
			match message {
				net_protocol::StatementDistributionMessage::V1(_) => return,
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::V1Compatibility(_),
				) => return,
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::Statement(
						relay_parent,
						statement,
					),
				) => {}, // TODO [now]
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(inner),
				) => {}, // TODO [now]
				net_protocol::StatementDistributionMessage::VStaging(
					protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(
						relay_parent,
						candidate_hash,
					),
				) => {}, // TODO [now]
			}
		},
		NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
			// TODO [now] update explicit and implicit views
		},
		NetworkBridgeEvent::OurViewChange(_view) => {
			// handled by `handle_activated_leaf`
		},
	}
}

/// If there is a new leaf, this should only be called for leaves which support
/// prospective parachains.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: ActiveLeavesUpdate,
) -> JfyiErrorResult<()> {
	if let Some(ref leaf) = update.activated {
		state
			.implicit_view
			.activate_leaf(ctx.sender(), leaf.hash)
			.await
			.map_err(JfyiError::ActivateLeafFailure)?;
	}

	handle_deactivate_leaves(state, &update.deactivated[..]);

	if let Some(ref leaf) = update.activated {
		// TODO [now]: determine which candidates are importable under the given
		// active leaf
	}

	for new_relay_parent in state.implicit_view.all_allowed_relay_parents() {
		if state.per_relay_parent.contains_key(new_relay_parent) {
			continue
		}

		// New leaf: fetch info from runtime API and initialize
		// `per_relay_parent`.
		let session_index = polkadot_node_subsystem_util::request_session_index_for_child(
			*new_relay_parent,
			ctx.sender(),
		)
		.await
		.await
		.map_err(JfyiError::RuntimeApiUnavailable)?
		.map_err(JfyiError::FetchSessionIndex)?;

		let availability_cores = polkadot_node_subsystem_util::request_availability_cores(
			*new_relay_parent,
			ctx.sender(),
		)
		.await
		.await
		.map_err(JfyiError::RuntimeApiUnavailable)?
		.map_err(JfyiError::FetchAvailabilityCores)?;

		if !state.per_session.contains_key(&session_index) {
			let session_info = polkadot_node_subsystem_util::request_session_info(
				*new_relay_parent,
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
				.insert(session_index, PerSessionState::new(session_info, &state.keystore).await);
		}

		let per_session = state
			.per_session
			.get(&session_index)
			.expect("either existed or just inserted; qed");

		let local_validator = per_session.local_validator.and_then(|v| {
			find_local_validator_state(
				v,
				&per_session.session_info.validators,
				&per_session.groups,
				&availability_cores,
			)
		});

		state.per_relay_parent.insert(
			*new_relay_parent,
			PerRelayParentState {
				validator_state: HashMap::new(),
				local_validator,
				statement_store: StatementStore::new(&per_session.groups),
				session: session_index,
			},
		);

		// TODO [now]: update peers which have the leaf in their view.
		// update their implicit view. send any messages accordingly.
	}

	Ok(())
}

fn find_local_validator_state(
	validator_index: ValidatorIndex,
	validators: &[ValidatorId],
	groups: &Groups,
	availability_cores: &[CoreState],
) -> Option<LocalValidatorState> {
	if groups.all().is_empty() {
		return None
	}

	let validator_id = validators.get(validator_index.0 as usize)?.clone();

	let our_group = groups.by_validator_index(validator_index)?;

	// note: this won't work well for parathreads because it only works
	// when core assignments to paras are static throughout the session.

	let para_for_group =
		|g: GroupIndex| availability_cores.get(g.0 as usize).and_then(|c| c.para_id());

	let group_validators = groups.get(our_group)?.to_owned();
	Some(LocalValidatorState {
		index: validator_index,
		group: our_group,
		assignment: para_for_group(our_group),
		cluster_tracker: ClusterTracker::new(
			group_validators,
			todo!(), // TODO [now]: seconding limit?
		)
		.expect("group is non-empty because we are in it; qed"),
		grid_tracker: GridTracker::default(),
	})
}

fn handle_deactivate_leaves(state: &mut State, leaves: &[Hash]) {
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
	state.per_relay_parent.retain(|r, x| relay_parents.contains(r));

	// TODO [now]: clean up requests
	state.candidates.on_deactivate_leaves(&leaves, |h| relay_parents.contains(h));

	// clean up sessions based on everything remaining.
	let sessions: HashSet<_> = state.per_relay_parent.values().map(|r| r.session).collect();
	state.per_session.retain(|s, _| sessions.contains(s));
}

// Imports a locally originating statement and distributes it to peers.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn share_local_statement<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	statement: SignedFullStatementWithPVD,
) -> JfyiErrorResult<()> {
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		None => return Err(JfyiError::InvalidShare),
		Some(x) => x,
	};

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

	let (expected_para, expected_relay_parent) = match expected {
		None => return Err(JfyiError::InvalidShare),
		Some(x) => x,
	};

	if local_index != statement.validator_index() {
		return Err(JfyiError::InvalidShare)
	}

	// TODO [now]: ensure seconded_count isn't too high. Needs our definition
	// of 'too high' i.e. max_depth, which isn't done yet.

	if local_assignment != Some(expected_para) || relay_parent != expected_relay_parent {
		return Err(JfyiError::InvalidShare)
	}

	// Insert candidate if unknown + more sanity checks.
	let (compact_statement, candidate_hash) = {
		let compact_statement = FullStatementWithPVD::signed_to_compact(statement.clone());
		let candidate_hash = CandidateHash(*statement.payload().candidate_hash());

		if let FullStatementWithPVD::Seconded(ref c, ref pvd) = statement.payload() {
			if let Some(reckoning) = state.candidates.confirm_candidate(
				candidate_hash,
				c.clone(),
				pvd.clone(),
				local_group,
			) {
				// TODO [now] apply the reckoning.
			}
		};

		match per_relay_parent
			.statement_store
			.insert(&per_session.groups, compact_statement.clone())
		{
			Ok(false) | Err(_) => {
				gum::warn!(
					target: LOG_TARGET,
					statement = ?compact_statement.payload(),
					"Candidate backing issued redundant statement?",
				);
				return Err(JfyiError::InvalidShare)
			},
			Ok(true) => (compact_statement, candidate_hash),
		}
	};

	// send the compact version of the statement to any peers which need it.
	circulate_statement(ctx, state, relay_parent, local_group, compact_statement).await;

	Ok(())
}

// two kinds of targets: those in our 'cluster' (currently just those in the same group),
// and those we are propagating to through the grid.
enum DirectTargetKind {
	Cluster,
	Grid,
}

// Circulates a compact statement to all peers who need it: those in the current group of the
// local validator, those in the next group for the parachain, and grid peers which have already
// indicated that they know the candidate as backed.
//
// If we're not sure whether the peer knows the candidate is `Seconded` already, we also send a `Seconded`
// statement.
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
	state: &mut State,
	relay_parent: Hash,
	group_index: GroupIndex,
	statement: SignedStatement,
) {
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(x) => x,
		None => return,
	};

	let per_session = match state.per_session.get(&per_relay_parent.session) {
		Some(s) => s,
		None => return,
	};
	let session_info = &per_session.session_info;

	let candidate_hash = statement.payload().candidate_hash().clone();

	let mut prior_seconded = None;
	let compact_statement = statement.payload().clone();
	let is_seconded = match compact_statement {
		CompactStatement::Seconded(_) => true,
		CompactStatement::Valid(_) => false,
	};

	let originator = statement.validator_index();
	let (local_validator, targets) = {
		let local_validator = match per_relay_parent.local_validator.as_mut() {
			Some(v) => v,
			None => return, // sanity: should be impossible to reach this.
		};

		let statement_group = per_session.groups.by_validator_index(originator);

		let cluster_relevant = Some(local_validator.group) == statement_group;
		let cluster_targets = if cluster_relevant {
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
			.direct_statement_recipients(&per_session.groups, originator, &compact_statement)
			.into_iter()
			.filter(|v| !cluster_relevant || !local_validator.cluster_tracker.targets().contains(v))
			.map(|v| (v, DirectTargetKind::Grid));

		let targets = cluster_targets
			.into_iter()
			.flat_map(|c| c)
			.chain(grid_targets)
			.filter_map(|(v, k)| {
				session_info.discovery_keys.get(v.0 as usize).map(|a| (v, a.clone(), k))
			})
			.collect::<Vec<_>>();

		(local_validator, targets)
	};

	let mut prior_to = Vec::new();
	let mut statement_to = Vec::new();
	for (target, authority_id, kind) in targets {
		// Find peer ID based on authority ID, and also filter to connected.
		let peer_id: PeerId = match state.authorities.get(&authority_id) {
			Some(p)
				if state.peers.get(p).map_or(false, |p| p.knows_relay_parent(&relay_parent)) =>
				p.clone(),
			None | Some(_) => continue,
		};

		match kind {
			DirectTargetKind::Cluster => {
				if !local_validator.cluster_tracker.knows_candidate(target, candidate_hash) &&
					!is_seconded
				{
					// lazily initialize this.
					let prior_seconded = if let Some(ref p) = prior_seconded.as_ref() {
						p
					} else {
						// This should always succeed because:
						// 1. If this is not a `Seconded` statement we must have
						//    received at least one `Seconded` statement from other validators
						//    in our cluster.
						// 2. We should have deposited all statements we've received into the statement store.

						match cluster_sendable_seconded_statement(
							&local_validator.cluster_tracker,
							&per_relay_parent.statement_store,
							candidate_hash,
						) {
							None => {
								gum::warn!(
									target: LOG_TARGET,
									?candidate_hash,
									?relay_parent,
									"degenerate state: we authored a `Valid` statement without \
									knowing any `Seconded` statements."
								);

								return
							},
							Some(s) => &*prior_seconded.get_or_insert(s.as_unchecked().clone()),
						}
					};

					// One of the properties of the 'cluster sendable seconded statement'
					// is that we `can_send` it to all nodes in the cluster which don't have the candidate already. And
					// we're already in a branch that's gated off from cluster nodes
					// which have knowledge of the candidate.
					local_validator.cluster_tracker.note_sent(
						target,
						prior_seconded.unchecked_validator_index(),
						CompactStatement::Seconded(candidate_hash),
					);
					prior_to.push(peer_id);
				}

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
				local_validator.grid_tracker.note_sent_or_received_direct_statement(
					&per_session.groups,
					originator,
					target,
					&compact_statement,
				);
			},
		}
	}

	// ship off the network messages to the network bridge.

	if !prior_to.is_empty() {
		let prior_seconded =
			prior_seconded.expect("prior_to is only non-empty when prior_seconded exists; qed");
		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			prior_to,
			Versioned::VStaging(protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				prior_seconded.clone(),
			))
			.into(),
		))
		.await;
	}

	if !statement_to.is_empty() {
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

fn cluster_sendable_seconded_statement<'a>(
	cluster_tracker: &ClusterTracker,
	statement_store: &'a StatementStore,
	candidate_hash: CandidateHash,
) -> Option<&'a SignedStatement> {
	cluster_tracker.sendable_seconder(candidate_hash).and_then(|v| {
		statement_store.validator_statement(v, CompactStatement::Seconded(candidate_hash))
	})
}

/// Check a statement signature under this parent hash.
fn check_statement_signature(
	session_index: SessionIndex,
	validators: &[ValidatorId],
	relay_parent: Hash,
	statement: UncheckedSignedStatement,
) -> std::result::Result<SignedStatement, UncheckedSignedStatement> {
	let signing_context = SigningContext { session_index, parent_hash: relay_parent };

	validators
		.get(statement.unchecked_validator_index().0 as usize)
		.ok_or_else(|| statement.clone())
		.and_then(|v| statement.try_into_checked(&signing_context, v))
}

async fn report_peer(
	sender: &mut impl overseer::StatementDistributionSenderTrait,
	peer: PeerId,
	rep: Rep,
) {
	sender.send_message(NetworkBridgeTxMessage::ReportPeer(peer, rep)).await
}

/// Handle an incoming statement.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn handle_incoming_statement<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer: PeerId,
	relay_parent: Hash,
	statement: UncheckedSignedStatement,
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
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE).await;
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
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		},
		Some(l) => l,
	};

	let originator_group =
		match per_session.groups.by_validator_index(statement.unchecked_validator_index()) {
			Some(g) => g,
			None => {
				report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
				return
			},
		};

	let cluster_sender_index = {
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

	let was_backed = false; // TODO [now]

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
				report_peer(ctx.sender(), peer, rep).await;
				return
			},
		}
	} else {
		let grid_sender_index = local_validator
			.grid_tracker
			.direct_statement_senders(
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
					report_peer(ctx.sender(), peer, rep).await;
					return
				},
			}
		} else {
			// Not a cluster or grid peer.
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		}
	};

	let statement = checked_statement.payload().clone();
	let originator_index = checked_statement.validator_index();
	let candidate_hash = *checked_statement.payload().candidate_hash();

	let originator_group = per_relay_parent
		.statement_store
		.validator_group_index(originator_index)
		.expect("validator confirmed to be known by statement_store.insert; qed");

	// Insert an unconfirmed candidate entry if needed
	{
		let res = state.candidates.insert_unconfirmed(
			peer.clone(),
			candidate_hash,
			relay_parent,
			originator_group,
			None,
		);

		if let Err(BadAdvertisement) = res {
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		}
	}

	if !state.candidates.is_confirmed(&candidate_hash) {
		// If the candidate is not confirmed, note that we should attempt
		// to request it from the given peer.
		let mut request_entry =
			state
				.request_manager
				.get_or_insert(relay_parent, candidate_hash, originator_group);

		request_entry.get_mut().add_peer(peer);

		// We only successfully accept statements from the grid on confirmed
		// candidates, therefore this check only passes if the statement is from the cluster
		request_entry.get_mut().set_cluster_priority();
	}

	let was_fresh =
		match per_relay_parent.statement_store.insert(&per_session.groups, checked_statement) {
			Err(_) => {
				// sanity: should never happen.
				gum::warn!(
					target: LOG_TARGET,
					?relay_parent,
					validator_index = ?originator_index,
					"Error - Cluster accepted message from unknown validator."
				);

				return
			},
			Ok(known) => known,
		};

	let is_backed = false; // TODO [now]

	if was_fresh {
		report_peer(ctx.sender(), peer, BENEFIT_VALID_STATEMENT_FIRST).await;

	// both of the below probably in some shared function.
	// TODO [now]: circulate the statement
	// TODO [now]: import the statement into backing if we can.
	} else {
		report_peer(ctx.sender(), peer, BENEFIT_VALID_STATEMENT).await;
	}

	if is_backed && !was_backed {
		// TODO [now]: handle a candidate being completely backed now.
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
	let checked_statement = match check_statement_signature(
		session,
		&session_info.validators[..],
		relay_parent,
		statement,
	) {
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
		&per_session.session_info.validators[..],
		relay_parent,
		statement,
	) {
		Ok(s) => s,
		Err(_) => return Err(COST_INVALID_SIGNATURE),
	};

	grid_tracker.note_sent_or_received_direct_statement(
		&per_session.groups,
		checked_statement.validator_index(),
		grid_sender_index,
		&checked_statement.payload(),
	);

	Ok(checked_statement)
}
