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
	self as net_protocol, peer_set::ValidationVersion, vstaging as protocol_vstaging,
	grid_topology::{RequiredRouting, SessionBoundGridTopologyStorage, SessionGridTopology},
	PeerId, View, Versioned,
};
use polkadot_node_primitives::{
	SignedFullStatementWithPVD,
	StatementWithPVD as FullStatementWithPVD,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{CandidateBackingMessage, NetworkBridgeEvent, NetworkBridgeTxMessage},
	overseer, ActivatedLeaf, PerLeafSpan, StatementDistributionSenderTrait,
};
use polkadot_node_subsystem_util::backing_implicit_view::{FetchError, View as ImplicitView};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, CommittedCandidateReceipt, CoreState, GroupIndex, Hash, Id as ParaId,
	PersistedValidationData, SignedStatement, UncheckedSignedStatement, ValidatorId,
	ValidatorIndex, CompactStatement, SessionIndex,
};

use sp_keystore::SyncCryptoStorePtr;

use indexmap::IndexMap;

use std::collections::{HashMap, HashSet};

use crate::{
	error::{JfyiError, JfyiErrorResult},
	LOG_TARGET,
};

struct PerRelayParentState {
	validators: Vec<ValidatorId>,
	discovery_keys: Vec<AuthorityDiscoveryId>,
	groups: Vec<Vec<ValidatorIndex>>,
	validator_state: HashMap<ValidatorIndex, PerRelayParentValidatorState>,
	candidates: HashMap<CandidateHash, CandidateData>,
	known_by: HashSet<PeerId>,
	local_validator: Option<LocalValidatorState>,
	session: SessionIndex,
}

struct PerRelayParentValidatorState {
	seconded_count: usize,
	group_id: GroupIndex,
}

// stores statements and the candidate receipt/persisted validation data if any.
struct CandidateData {
	state: CandidateState,
	seconded_statements: Vec<SignedStatement>,
	valid_statements: Vec<SignedStatement>,

	// validators which have either produced a statement about the
	// candidate or which have sent a signed statement or which we have
	// sent statements to.
	known_by: HashSet<ValidatorIndex>,
}

impl Default for CandidateData {
	fn default() -> Self {
		CandidateData {
			state: CandidateState::Unconfirmed,
			seconded_statements: Vec::new(),
			valid_statements: Vec::new(),
			known_by: HashSet::new(),
		}
	}
}

impl CandidateData {
	fn has_issued_seconded(&self, validator: ValidatorIndex) -> bool {
		self.seconded_statements.iter().find(|s| s.validator_index() == validator).is_some()
	}

	fn has_issued_valid(&self, validator: ValidatorIndex) -> bool {
		self.valid_statements.iter().find(|s| s.validator_index() == validator).is_some()
	}

	// ignores duplicates or equivocations. returns 'false' if those are detected, 'true' otherwise.
	fn insert_signed_statement(&mut self, statement: SignedStatement) -> bool {
		let validator_index = statement.validator_index();

		// only accept one statement by the validator.
		let has_issued_statement = self.has_issued_seconded(validator_index) || self.has_issued_valid(validator_index);
		if has_issued_statement { return false }

		match statement.payload() {
			CompactStatement::Seconded(_) => self.seconded_statements.push(statement),
			CompactStatement::Valid(_) => self.valid_statements.push(statement),
		}

		self.known_by.insert(validator_index);

		true
	}

	fn note_known_by(&mut self, validator: ValidatorIndex) {
		self.known_by.insert(validator);
	}
}

enum CandidateState {
	/// The candidate is unconfirmed to exist, as it hasn't yet
	/// been fetched.
	Unconfirmed,
	/// The candidate is confirmed and we have the `PersistedValidationData`.
	Confirmed(CommittedCandidateReceipt, PersistedValidationData),
}

impl CandidateState {
	fn is_confirmed(&self) -> bool {
		match *self {
			CandidateState::Unconfirmed => false,
			CandidateState::Confirmed(_, _) => true,
		}
	}

	fn receipt(&self) -> Option<&CommittedCandidateReceipt> {
		match *self {
			CandidateState::Unconfirmed => None,
			CandidateState::Confirmed(ref c, _) => Some(c),
		}
	}
}

// per-relay-parent local validator state.
struct LocalValidatorState {
	// The index of the validator.
	index: ValidatorIndex,
	// our validator group
	group: GroupIndex,
	// the assignment of our validator group, if any.
	assignment: Option<ParaId>,
	// the next group assigned to this para.
	next_group: GroupIndex,
	// the previous group assigned to this para, stored only
	// if they are currently assigned to a para.
	prev_group: Option<(GroupIndex, ParaId)>,
}

pub(crate) struct State {
	/// The utility for managing the implicit and explicit views in a consistent way.
	///
	/// We only feed leaves which have prospective parachains enabled to this view.
	implicit_view: ImplicitView,
	per_relay_parent: HashMap<Hash, PerRelayParentState>,
	peers: HashMap<PeerId, PeerState>,
	keystore: SyncCryptoStorePtr,
	topology_storage: SessionBoundGridTopologyStorage,
}

struct PeerState {
	view: View,
}

#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
pub(crate) async fn handle_network_update<Context>(
	ctx: &mut Context,
	state: &mut State,
	update: NetworkBridgeEvent<net_protocol::StatementDistributionMessage>,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer_id, _role, protocol_version, authority_ids) => {
			if protocol_version != ValidationVersion::VStaging.into() {
				return
			}

			state.peers.insert(peer_id, PeerState { view: View::default() });

			// TODO [now]: update some authorities map.
		},
		NetworkBridgeEvent::PeerDisconnected(peer_id) => {
			state.peers.remove(&peer_id);
		},
		NetworkBridgeEvent::NewGossipTopology(new_topology) => {
			// TODO [now]
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
					protocol_vstaging::StatementDistributionMessage::BackedCandidateInventory(
						inner,
					),
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
			// TODO [now]
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

		let session_info =
			polkadot_node_subsystem_util::request_session_info(*leaf, session_index, ctx.sender())
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

		let availability_cores =
			polkadot_node_subsystem_util::request_availability_cores(*leaf, ctx.sender())
				.await
				.await
				.map_err(JfyiError::RuntimeApiUnavailable)?
				.map_err(JfyiError::FetchAvailabilityCores)?;

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
				validators: session_info.validators,
				discovery_keys: session_info.discovery_keys,
				groups: session_info.validator_groups,
				validator_state: HashMap::new(),
				candidates: HashMap::new(),
				known_by: HashSet::new(),
				local_validator,
				session: session_index,
			},
		);
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

	let next_group = GroupIndex((our_group.0 + 1) % groups.len() as u32);
	let prev_group =
		GroupIndex(if our_group.0 == 0 { our_group.0 - 1 } else { groups.len() as u32 - 1 });

	let para_for_group =
		|g: GroupIndex| availability_cores.get(g.0 as usize).and_then(|c| c.para_id());

	Some(LocalValidatorState {
		index: validator_index,
		group: our_group,
		assignment: para_for_group(our_group),
		next_group,
		prev_group: para_for_group(prev_group).map(|p| (prev_group, p)),
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
	state.per_relay_parent.retain(|r, _| relay_parents.contains(r));
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

	let (local_index, local_assignment) = match per_relay_parent.local_validator.as_ref() {
		None => return Err(JfyiError::InvalidShare),
		Some(l) => (l.index, l.assignment)
	};

	// Two possibilities: either the statement is `Seconded` or we already
	// have the candidate. Sanity: check the para-id is valid.
	let expected_para = match statement.payload() {
		FullStatementWithPVD::Seconded(ref c, _) => Some(c.descriptor().para_id),
		FullStatementWithPVD::Valid(hash) => per_relay_parent
			.candidates
			.get(&hash)
			.and_then(|c| c.state.receipt())
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
				let candidate_entry = per_relay_parent.candidates.entry(candidate_hash).or_default();

				if let CandidateState::Unconfirmed = candidate_entry.state {
					candidate_entry.state = CandidateState::Confirmed(c.clone(), pvd.clone());
				}

				candidate_entry
			}
			FullStatementWithPVD::Valid(_) => {
				match per_relay_parent.candidates.get_mut(&candidate_hash) {
					None  => {
						// Can't share a 'Valid' statement about a candidate we don't know about!
						return Err(JfyiError::InvalidShare);
					}
					Some(ref c) if !c.state.is_confirmed() => {
						// Can't share a 'Valid' statement about a candidate we don't know about!
						return Err(JfyiError::InvalidShare);
					}
					Some(c) => c,
				}
			}
		};

		if !candidate_entry.insert_signed_statement(compact_statement.clone()) {
			gum::warn!(
				target: LOG_TARGET,
				statement = ?compact_statement.payload(),
				"Candidate backing issued redundant statement?",
			);

			return Err(JfyiError::InvalidShare);
		}

		(compact_statement, candidate_hash)
	};

	// TODO [now]:
	// 3. send the compact version of the statement to nodes in current group and next-up. If not a `Seconded` statement,
	//   send a `Seconded` statement as well.
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
// preconditions: the candidate entry exists in the state under the relay parent
// and the statement has already been imported into the entry. If this is a `Valid`
// statement, then there must be at least one `Seconded` statement.
// TODO [now]: make this a more general `broadcast_statement` with an `BroadcastBehavior` that
// affects targets: `Local` keeps current behavior while `Forward` only sends onwards via `BackedCandidate` knowers.
#[overseer::contextbounds(StatementDistribution, prefix=self::overseer)]
async fn broadcast_local_statement<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	statement: SignedStatement,
) {
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(x) => x,
		None => return,
	};

	let candidate_hash = statement.payload().candidate_hash().clone();
	let candidate_entry = match per_relay_parent.candidates.get_mut(&candidate_hash) {
		Some(x) => x,
		None => return,
	};

	let prior_seconded = match statement.payload() {
		CompactStatement::Seconded(_) => None,
		CompactStatement::Valid(_) => match candidate_entry.seconded_statements.first() {
			Some(s) => Some(s.as_unchecked().clone()),
			None => return,
		}
	};

	let targets = {
		let local_validator = match per_relay_parent.local_validator.as_ref() {
			Some(v) => v,
			None => return, // sanity: should be impossible to reach this.
		};

		let current_group = per_relay_parent.groups[local_validator.group.0 as usize].iter().cloned();
		let next_group = per_relay_parent.groups[local_validator.next_group.0 as usize].iter().cloned();

		// TODO [now]: extend targets with validators which
		// 	a) we've sent `BackedCandidateInv` for this candidate to
		//	b) have either requested the candidate _or_ have sent `BackedCandidateKnown` to us.

		current_group.chain(next_group)
			.filter_map(|v| per_relay_parent.discovery_keys.get(v.0 as usize).map(|a| (v, a.clone())))
			.collect::<Vec<_>>()
	};

	let mut prior_to = Vec::new();
	let mut statement_to = Vec::new();
	for (validator_index, authority_id) in targets {
		// TODO [now], also continue if not connected.
		let peer_id: PeerId = unimplemented!();

		// We guarantee that the receiving peer knows the candidate by
		// sending them a `Seconded` statement first.
		if candidate_entry.known_by.insert(validator_index) {
			if let Some(_) = prior_seconded.as_ref() {
				prior_to.push(peer_id.clone());
			}
		}

		statement_to.push(peer_id);
	}

	// ship off the network messages to the network bridge.

	if !prior_to.is_empty() {
		let prior_seconded = prior_seconded.expect("prior_to is only non-empty when prior_seconded exists; qed");
		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			prior_to,
			Versioned::VStaging(protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				prior_seconded.clone(),
			)).into()
		)).await;
	}

	if !statement_to.is_empty() {
		ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
			statement_to,
			Versioned::VStaging(protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				statement.as_unchecked().clone(),
			)).into()
		)).await;
	}
}
