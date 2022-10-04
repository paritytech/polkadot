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
	grid_topology::{RequiredRouting, SessionGridTopologies, SessionGridTopology},
	peer_set::ValidationVersion,
	vstaging as protocol_vstaging, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{
	SignedFullStatementWithPVD, StatementWithPVD as FullStatementWithPVD,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{CandidateBackingMessage, NetworkBridgeEvent, NetworkBridgeTxMessage},
	overseer, ActivatedLeaf, PerLeafSpan, StatementDistributionSenderTrait,
};
use polkadot_node_subsystem_util::backing_implicit_view::{FetchError, View as ImplicitView};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, CommittedCandidateReceipt, CompactStatement, CoreState,
	GroupIndex, Hash, Id as ParaId, PersistedValidationData, SessionIndex, SessionInfo,
	SignedStatement, SigningContext, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
};

use sp_keystore::SyncCryptoStorePtr;

use indexmap::IndexMap;

use std::collections::{HashMap, HashSet};

use crate::{
	error::{JfyiError, JfyiErrorResult},
	LOG_TARGET,
};
use candidate_entry::CandidateEntry;
use cluster::{Accept as ClusterAccept, ClusterTracker, RejectIncoming as ClusterRejectIncoming};
use statement_store::StatementStore;

mod candidate_entry;
mod cluster;
mod requester;
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

const BENEFIT_VALID_RESPONSE: Rep = Rep::BenefitMajor("Peer Answered Candidate Request");

struct PerRelayParentState {
	validator_state: HashMap<ValidatorIndex, PerRelayParentValidatorState>,
	candidates: HashMap<CandidateHash, CandidateEntry>,
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
}

pub(crate) struct State {
	/// The utility for managing the implicit and explicit views in a consistent way.
	///
	/// We only feed leaves which have prospective parachains enabled to this view.
	implicit_view: ImplicitView,
	per_relay_parent: HashMap<Hash, PerRelayParentState>,
	per_session: HashMap<SessionIndex, SessionInfo>,
	peers: HashMap<PeerId, PeerState>,
	keystore: SyncCryptoStorePtr,
	topology_storage: SessionGridTopologies,
	authorities: HashMap<AuthorityDiscoveryId, PeerId>,
}

struct PeerState {
	view: View,
	implicit_view: HashSet<Hash>,
	maybe_authority: Option<HashSet<AuthorityDiscoveryId>>,
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
		self.maybe_authority.as_ref().map_or(false, |x| x.contains(authority_id))
	}
}

/// How many votes we need to consider a candidate backed.
///
/// WARNING: This has to be kept in sync with the runtime check in the inclusion module.
fn minimum_votes(n_validators: usize) -> usize {
	std::cmp::min(2, n_validators)
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_network_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: NetworkBridgeEvent<net_protocol::StatementDistributionMessage>,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer_id, role, protocol_version, authority_ids) => {
			gum::trace!(target: LOG_TARGET, ?peer_id, ?role, ?protocol_version, "Peer connected");

			if protocol_version != ValidationVersion::VStaging.into() {
				return
			}

			state.peers.insert(
				peer_id,
				PeerState {
					view: View::default(),
					implicit_view: HashSet::new(),
					maybe_authority: authority_ids.clone(),
				},
			);

			if let Some(authority_ids) = authority_ids {
				authority_ids.into_iter().for_each(|a| {
					state.authorities.insert(a, peer_id);
				})
			}
		},
		NetworkBridgeEvent::PeerDisconnected(peer_id) => {
			state.peers.remove(&peer_id);
		},
		NetworkBridgeEvent::NewGossipTopology(topology) => {
			let new_session_index = topology.session;
			let new_topology: SessionGridTopology = topology.into();
			state.topology_storage.insert_topology(new_session_index, new_topology);

			// TODO [now]: can we not update authority IDs for peers?

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

/// This should only be invoked for leaves that implement prospective parachains.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_activated_leaf<Context>(
	ctx: &mut Context,
	state: &mut State,
	leaf: ActivatedLeaf,
) -> JfyiErrorResult<()> {
	state
		.implicit_view
		.activate_leaf(ctx.sender(), leaf.hash)
		.await
		.map_err(JfyiError::ActivateLeafFailure)?;

	for leaf in state.implicit_view.all_allowed_relay_parents() {
		if state.per_relay_parent.contains_key(leaf) {
			continue
		}

		// New leaf: fetch info from runtime API and initialize
		// `per_relay_parent`.
		let session_index =
			polkadot_node_subsystem_util::request_session_index_for_child(*leaf, ctx.sender())
				.await
				.await
				.map_err(JfyiError::RuntimeApiUnavailable)?
				.map_err(JfyiError::FetchSessionIndex)?;

		let availability_cores =
			polkadot_node_subsystem_util::request_availability_cores(*leaf, ctx.sender())
				.await
				.await
				.map_err(JfyiError::RuntimeApiUnavailable)?
				.map_err(JfyiError::FetchAvailabilityCores)?;

		if !state.per_session.contains_key(&session_index) {
			let session_info = polkadot_node_subsystem_util::request_session_info(
				*leaf,
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
						relay_parent = ?leaf,
						"No session info available for current session"
					);

					continue
				},
				Some(s) => s,
			};

			state.per_session.insert(session_index, session_info);
		}

		let session_info = state
			.per_session
			.get(&session_index)
			.expect("either existed or just inserted; qed");

		let local_validator = find_local_validator_state(
			&session_info.validators,
			&state.keystore,
			&session_info.validator_groups,
			&availability_cores,
		)
		.await;

		state.per_relay_parent.insert(
			*leaf,
			PerRelayParentState {
				validator_state: HashMap::new(),
				candidates: HashMap::new(),
				local_validator,
				statement_store: StatementStore::new(session_info.validator_groups.clone()),
				session: session_index,
			},
		);

		state.topology_storage.inc_session_refs(session_index);

		// TODO [now]: update peers which have the leaf in their view.
		// update their implicit view. send any messages accordingly.
	}

	Ok(())
}

async fn find_local_validator_state(
	validators: &[ValidatorId],
	keystore: &SyncCryptoStorePtr,
	groups: &[Vec<ValidatorIndex>],
	availability_cores: &[CoreState],
) -> Option<LocalValidatorState> {
	if groups.is_empty() {
		return None
	}

	let (validator_id, validator_index) =
		polkadot_node_subsystem_util::signing_key_and_index(validators, keystore).await?;

	let our_group = polkadot_node_subsystem_util::find_validator_group(groups, validator_index)?;

	// note: this won't work well for parathreads because it only works
	// when core assignments to paras are static throughout the session.

	let para_for_group =
		|g: GroupIndex| availability_cores.get(g.0 as usize).and_then(|c| c.para_id());

	let group_validators = groups[our_group.0 as usize].clone();
	Some(LocalValidatorState {
		index: validator_index,
		group: our_group,
		assignment: para_for_group(our_group),
		cluster_tracker: ClusterTracker::new(
			group_validators,
			todo!(), // TODO [now]: seconding limit?
		)
		.expect("group is non-empty because we are in it; qed"),
	})
}

pub(crate) fn handle_deactivate_leaf(state: &mut State, leaf_hash: Hash) {
	// deactivate the leaf in the implicit view.
	state.implicit_view.deactivate_leaf(leaf_hash);
	let relay_parents = state.implicit_view.all_allowed_relay_parents().collect::<HashSet<_>>();

	// fast exit for no-op.
	if relay_parents.len() == state.per_relay_parent.len() {
		return
	}

	// clean up per-relay-parent data based on everything removed.
	let topology_storage = &mut state.topology_storage;
	state.per_relay_parent.retain(|r, x| {
		if relay_parents.contains(r) {
			true
		} else {
			// clean up topology storage.
			topology_storage.dec_session_refs(x.session);

			false
		}
	});

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

	let (local_index, local_assignment, local_group) =
		match per_relay_parent.local_validator.as_ref() {
			None => return Err(JfyiError::InvalidShare),
			Some(l) => (l.index, l.assignment, l.group),
		};

	// Two possibilities: either the statement is `Seconded` or we already
	// have the candidate. Sanity: check the para-id is valid.
	let expected_para = match statement.payload() {
		FullStatementWithPVD::Seconded(ref c, _) => Some(c.descriptor().para_id),
		FullStatementWithPVD::Valid(hash) => per_relay_parent
			.candidates
			.get(&hash)
			.and_then(|c| c.receipt())
			.map(|c| c.descriptor().para_id),
	};

	if local_index != statement.validator_index() {
		return Err(JfyiError::InvalidShare)
	}

	// TODO [now]: ensure seconded_count isn't too high. Needs our definition
	// of 'too high' i.e. max_depth, which isn't done yet.

	if expected_para.is_none() || local_assignment != expected_para {
		return Err(JfyiError::InvalidShare)
	}

	// Insert candidate if unknown + more sanity checks.
	let (compact_statement, candidate_hash) = {
		let compact_statement = FullStatementWithPVD::signed_to_compact(statement.clone());
		let candidate_hash = CandidateHash(*statement.payload().candidate_hash());

		let candidate_entry = match statement.payload() {
			FullStatementWithPVD::Seconded(ref c, ref pvd) => {
				let candidate_entry =
					per_relay_parent.candidates.entry(candidate_hash).or_insert_with(|| {
						CandidateEntry::confirmed(candidate_hash, c.clone(), pvd.clone())
					});

				candidate_entry
			},
			FullStatementWithPVD::Valid(_) => {
				match per_relay_parent.candidates.get_mut(&candidate_hash) {
					None => {
						// Can't share a 'Valid' statement about a candidate we don't know about!
						return Err(JfyiError::InvalidShare)
					},
					Some(ref c) if !c.is_confirmed() => {
						// Can't share a 'Valid' statement about a candidate we don't know about!
						return Err(JfyiError::InvalidShare)
					},
					Some(c) => c,
				}
			},
		};

		if !per_relay_parent.statement_store.insert(compact_statement.clone()) {
			gum::warn!(
				target: LOG_TARGET,
				statement = ?compact_statement.payload(),
				"Candidate backing issued redundant statement?",
			);
			return Err(JfyiError::InvalidShare)
		}

		(compact_statement, candidate_hash)
	};

	// send the compact version of the statement to nodes in current group and next-up. If not a `Seconded` statement,
	// send a `Seconded` statement as well.
	send_statement_direct(ctx, state, relay_parent, local_group, compact_statement).await;

	// TODO [now]:
	// 4. If the candidate is now backed, trigger 'backed candidate announcement' logic.

	Ok(())
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
// TODO [now]: make this a more general `broadcast_statement` with an `BroadcastBehavior` that
// affects targets: `Local` keeps current behavior while `Forward` only sends onwards via `BackedCandidate` knowers.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn send_statement_direct<Context>(
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

	let session_info = match state.per_session.get(&per_relay_parent.session) {
		Some(s) => s,
		None => return,
	};

	let candidate_hash = statement.payload().candidate_hash().clone();

	let mut prior_seconded = None;
	let compact_statement = statement.payload().clone();
	let is_seconded = match compact_statement {
		CompactStatement::Seconded(_) => true,
		CompactStatement::Valid(_) => false,
	};

	// two kinds of targets: those in our 'cluster' (currently just those in the same group),
	// and those we are propagating to through the grid.
	enum TargetKind {
		Cluster,
		Grid,
	}

	let (local_validator, targets) = {
		let local_validator = match per_relay_parent.local_validator.as_mut() {
			Some(v) => v,
			None => return, // sanity: should be impossible to reach this.
		};

		let current_group = local_validator
			.cluster_tracker
			.targets()
			.iter()
			.filter(|&v| v != &local_validator.index)
			.map(|v| (*v, TargetKind::Cluster));

		// TODO [now]: extend with grid targets, dedup

		let targets = current_group
			.filter_map(|(v, k)| {
				session_info.discovery_keys.get(v.0 as usize).map(|a| (v, a.clone(), k))
			})
			.collect::<Vec<_>>();

		(local_validator, targets)
	};

	let originator = statement.validator_index();

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
			TargetKind::Cluster => {
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
			TargetKind::Grid => {
				// TODO [now]
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

	let session_info = match state.per_session.get(&per_relay_parent.session) {
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

	let local_validator = match per_relay_parent.local_validator.as_mut() {
		None => {
			// we shouldn't be receiving statements unless we're a validator
			// this session.
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		},
		Some(l) => l,
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

	// TODO [now]: handle direct statements from grid peers
	let cluster_sender_index = match cluster_sender_index {
		None => {
			report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
			return
		},
		Some(c) => c,
	};

	// additional cluster checks.
	{
		match local_validator.cluster_tracker.can_receive(
			cluster_sender_index,
			statement.unchecked_validator_index(),
			statement.unchecked_payload().clone(),
		) {
			Ok(ClusterAccept::Ok | ClusterAccept::WithPrejudice) => {},
			Err(ClusterRejectIncoming::ExcessiveSeconded) => {
				report_peer(ctx.sender(), peer, COST_EXCESSIVE_SECONDED).await;
				return
			},
			Err(ClusterRejectIncoming::CandidateUnknown | ClusterRejectIncoming::Duplicate) => {
				report_peer(ctx.sender(), peer, COST_UNEXPECTED_STATEMENT).await;
				return
			},
			Err(ClusterRejectIncoming::NotInGroup) => {
				// sanity: shouldn't be possible; we already filtered this
				// out above.
				return
			},
		}
	}

	// Ensure the statement is correctly signed.
	let checked_statement = match check_statement_signature(
		per_relay_parent.session,
		&session_info.validators[..],
		relay_parent,
		statement,
	) {
		Ok(s) => s,
		Err(_) => {
			report_peer(ctx.sender(), peer, COST_INVALID_SIGNATURE).await;
			return
		},
	};

	local_validator.cluster_tracker.note_received(
		cluster_sender_index,
		checked_statement.validator_index(),
		checked_statement.payload().clone(),
	);

	if !per_relay_parent.statement_store.insert(checked_statement) {
		return
	}

	// TODO [now]:
	// * add a candidate entry if we need to
	// * issue requests for the candidate if we need to
	// * import the statement into backing if we can.
}
