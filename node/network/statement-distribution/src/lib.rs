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

//! The Statement Distribution Subsystem.
//!
//! This is responsible for distributing signed statements about candidate
//! validity amongst validators.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use polkadot_subsystem::{
	Subsystem, SubsystemResult, SubsystemContext, SpawnedSubsystem,
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, PerLeafSpan,
	jaeger,
	messages::{
		AllMessages, NetworkBridgeMessage, StatementDistributionMessage, CandidateBackingMessage,
		RuntimeApiMessage, RuntimeApiRequest, NetworkBridgeEvent,
	},
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_node_primitives::{SignedFullStatement};
use polkadot_primitives::v1::{
	Hash, CompactStatement, ValidatorIndex, ValidatorId, SigningContext, ValidatorSignature, CandidateHash,
};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, View, PeerId, OurView, UnifiedReputationChange as Rep,
};

use futures::prelude::*;
use futures::channel::oneshot;
use indexmap::IndexSet;

use std::collections::{HashMap, HashSet};

const COST_UNEXPECTED_STATEMENT: Rep = Rep::CostMinor("Unexpected Statement");
const COST_INVALID_SIGNATURE: Rep = Rep::CostMajor("Invalid Statement Signature");
const COST_DUPLICATE_STATEMENT: Rep = Rep::CostMajorRepeated("Statement sent more than once by peer");
const COST_APPARENT_FLOOD: Rep = Rep::Malicious("Peer appears to be flooding us with statements");

const BENEFIT_VALID_STATEMENT: Rep = Rep::BenefitMajor("Peer provided a valid statement");
const BENEFIT_VALID_STATEMENT_FIRST: Rep = Rep::BenefitMajorFirst(
	"Peer was the first to provide a valid statement",
);

/// The maximum amount of candidates each validator is allowed to second at any relay-parent.
/// Short for "Validator Candidate Threshold".
///
/// This is the amount of candidates we keep per validator at any relay-parent.
/// Typically we will only keep 1, but when a validator equivocates we will need to track 2.
const VC_THRESHOLD: usize = 2;

const LOG_TARGET: &str = "statement_distribution";

/// The statement distribution subsystem.
pub struct StatementDistribution {
	// Prometheus metrics
	metrics: Metrics,
}

impl<C> Subsystem<C> for StatementDistribution
	where C: SubsystemContext<Message=StatementDistributionMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run`.
		SpawnedSubsystem {
			name: "statement-distribution-subsystem",
			future: self.run(ctx).boxed(),
		}
	}
}

impl StatementDistribution {
	/// Create a new Statement Distribution Subsystem
	pub fn new(metrics: Metrics) -> StatementDistribution {
		StatementDistribution {
			metrics,
		}
	}
}

/// Tracks our impression of a single peer's view of the candidates a validator has seconded
/// for a given relay-parent.
///
/// It is expected to receive at most `VC_THRESHOLD` from us and be aware of at most `VC_THRESHOLD`
/// via other means.
#[derive(Default)]
struct VcPerPeerTracker {
	local_observed: arrayvec::ArrayVec<[CandidateHash; VC_THRESHOLD]>,
	remote_observed: arrayvec::ArrayVec<[CandidateHash; VC_THRESHOLD]>,
}

impl VcPerPeerTracker {
	/// Note that the remote should now be aware that a validator has seconded a given candidate (by hash)
	/// based on a message that we have sent it from our local pool.
	fn note_local(&mut self, h: CandidateHash) {
		if !note_hash(&mut self.local_observed, h) {
			tracing::warn!("Statement distribution is erroneously attempting to distribute more \
				than {} candidate(s) per validator index. Ignoring", VC_THRESHOLD);
		}
	}

	/// Note that the remote should now be aware that a validator has seconded a given candidate (by hash)
	/// based on a message that it has sent us.
	///
	/// Returns `true` if the peer was allowed to send us such a message, `false` otherwise.
	fn note_remote(&mut self, h: CandidateHash) -> bool {
		note_hash(&mut self.remote_observed, h)
	}
}

fn note_hash(
	observed: &mut arrayvec::ArrayVec<[CandidateHash; VC_THRESHOLD]>,
	h: CandidateHash,
) -> bool {
	if observed.contains(&h) { return true; }

	observed.try_push(h).is_ok()
}

/// knowledge that a peer has about goings-on in a relay parent.
#[derive(Default)]
struct PeerRelayParentKnowledge {
	/// candidates that the peer is aware of. This indicates that we can
	/// send other statements pertaining to that candidate.
	known_candidates: HashSet<CandidateHash>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to the peer by us.
	sent_statements: HashSet<(CompactStatement, ValidatorIndex)>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to us by the peer.
	received_statements: HashSet<(CompactStatement, ValidatorIndex)>,
	/// How many candidates this peer is aware of for each given validator index.
	seconded_counts: HashMap<ValidatorIndex, VcPerPeerTracker>,
	/// How many statements we've received for each candidate that we're aware of.
	received_message_count: HashMap<CandidateHash, usize>,
}

impl PeerRelayParentKnowledge {
	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based
	/// on something that we would like to send to the peer.
	///
	/// This returns `None` if the peer cannot accept this statement, without altering internal
	/// state.
	///
	/// If the peer can accept the statement, this returns `Some` and updates the internal state.
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	///
	/// This returns `Some(true)` if this is the first time the peer has become aware of a
	/// candidate with the given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn send(&mut self, fingerprint: &(CompactStatement, ValidatorIndex)) -> Option<bool> {
		let already_known = self.sent_statements.contains(fingerprint)
			|| self.received_statements.contains(fingerprint);

		if already_known {
			return None;
		}

		let new_known = match fingerprint.0 {
			CompactStatement::Candidate(ref h) => {
				self.seconded_counts.entry(fingerprint.1)
					.or_default()
					.note_local(h.clone());

				self.known_candidates.insert(h.clone())
			},
			CompactStatement::Valid(ref h) | CompactStatement::Invalid(ref h) => {
				// The peer can only accept Valid and Invalid statements for which it is aware
				// of the corresponding candidate.
				if !self.known_candidates.contains(h) {
					return None;
				}

				false
			}
		};

		self.sent_statements.insert(fingerprint.clone());

		Some(new_known)
	}

	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based on
	/// a message we are receiving from the peer.
	///
	/// Provide the maximum message count that we can receive per candidate. In practice we should
	/// not receive more statements for any one candidate than there are members in the group assigned
	/// to that para, but this maximum needs to be lenient to account for equivocations that may be
	/// cross-group. As such, a maximum of 2 * n_validators is recommended.
	///
	/// This returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	///
	/// If this returns `Ok`, the internal state has been altered. After `receive`ing a new
	/// candidate, we are then cleared to send the peer further statements about that candidate.
	///
	/// This returns `Ok(true)` if this is the first time the peer has become aware of a
	/// candidate with given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn receive(
		&mut self,
		fingerprint: &(CompactStatement, ValidatorIndex),
		max_message_count: usize,
	) -> Result<bool, Rep> {
		// We don't check `sent_statements` because a statement could be in-flight from both
		// sides at the same time.
		if self.received_statements.contains(fingerprint) {
			return Err(COST_DUPLICATE_STATEMENT);
		}

		let candidate_hash = match fingerprint.0 {
			CompactStatement::Candidate(ref h) => {
				let allowed_remote = self.seconded_counts.entry(fingerprint.1)
					.or_insert_with(Default::default)
					.note_remote(h.clone());

				if !allowed_remote {
					return Err(COST_UNEXPECTED_STATEMENT);
				}

				h
			}
			CompactStatement::Valid(ref h)| CompactStatement::Invalid(ref h) => {
				if !self.known_candidates.contains(&h) {
					return Err(COST_UNEXPECTED_STATEMENT);
				}

				h
			}
		};

		{
			let received_per_candidate = self.received_message_count
				.entry(*candidate_hash)
				.or_insert(0);

			if *received_per_candidate >= max_message_count {
				return Err(COST_APPARENT_FLOOD);
			}

			*received_per_candidate += 1;
		}

		self.received_statements.insert(fingerprint.clone());
		Ok(self.known_candidates.insert(candidate_hash.clone()))
	}
}

struct PeerData {
	view: View,
	view_knowledge: HashMap<Hash, PeerRelayParentKnowledge>,
}

impl PeerData {
	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based
	/// on something that we would like to send to the peer.
	///
	/// This returns `None` if the peer cannot accept this statement, without altering internal
	/// state.
	///
	/// If the peer can accept the statement, this returns `Some` and updates the internal state.
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	///
	/// This returns `Some(true)` if this is the first time the peer has become aware of a
	/// candidate with the given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn send(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
	) -> Option<bool> {
		self.view_knowledge.get_mut(relay_parent).map_or(None, |k| k.send(fingerprint))
	}

	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based on
	/// a message we are receiving from the peer.
	///
	/// Provide the maximum message count that we can receive per candidate. In practice we should
	/// not receive more statements for any one candidate than there are members in the group assigned
	/// to that para, but this maximum needs to be lenient to account for equivocations that may be
	/// cross-group. As such, a maximum of 2 * n_validators is recommended.
	///
	/// This returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	///
	/// If this returns `Ok`, the internal state has been altered. After `receive`ing a new
	/// candidate, we are then cleared to send the peer further statements about that candidate.
	///
	/// This returns `Ok(true)` if this is the first time the peer has become aware of a
	/// candidate with given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn receive(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
		max_message_count: usize,
	) -> Result<bool, Rep> {
		self.view_knowledge
			.get_mut(relay_parent)
			.ok_or(COST_UNEXPECTED_STATEMENT)?
			.receive(fingerprint, max_message_count)
	}
}

// A statement stored while a relay chain head is active.
#[derive(Debug)]
struct StoredStatement {
	comparator: StoredStatementComparator,
	statement: SignedFullStatement,
}

// A value used for comparison of stored statements to each other.
//
// The compact version of the statement, the validator index, and the signature of the validator
// is enough to differentiate between all types of equivocations, as long as the signature is
// actually checked to be valid. The same statement with 2 signatures and 2 statements with
// different (or same) signatures wll all be correctly judged to be unequal with this comparator.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct StoredStatementComparator {
	compact: CompactStatement,
	validator_index: ValidatorIndex,
	signature: ValidatorSignature,
}

impl StoredStatement {
	fn compact(&self) -> &CompactStatement {
		&self.comparator.compact
	}

	fn fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
		(self.comparator.compact.clone(), self.statement.validator_index())
	}
}

impl std::borrow::Borrow<StoredStatementComparator> for StoredStatement {
	fn borrow(&self) -> &StoredStatementComparator {
		&self.comparator
	}
}

impl std::hash::Hash for StoredStatement {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.comparator.hash(state)
	}
}

impl std::cmp::PartialEq for StoredStatement {
	fn eq(&self, other: &Self) -> bool {
		&self.comparator == &other.comparator
	}
}

impl std::cmp::Eq for StoredStatement {}

#[derive(Debug)]
enum NotedStatement<'a> {
	NotUseful,
	Fresh(&'a StoredStatement),
	UsefulButKnown
}

struct ActiveHeadData {
	/// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<CandidateHash>,
	/// Stored statements for circulation to peers.
	///
	/// These are iterable in insertion order, and `Seconded` statements are always
	/// accepted before dependent statements.
	statements: IndexSet<StoredStatement>,
	/// The validators at this head.
	validators: Vec<ValidatorId>,
	/// The session index this head is at.
	session_index: sp_staking::SessionIndex,
	/// How many `Seconded` statements we've seen per validator.
	seconded_counts: HashMap<ValidatorIndex, usize>,
	/// A Jaeger span for this head, so we can attach data to it.
	span: PerLeafSpan,
}

impl ActiveHeadData {
	fn new(
		validators: Vec<ValidatorId>,
		session_index: sp_staking::SessionIndex,
		span: PerLeafSpan,
	) -> Self {
		ActiveHeadData {
			candidates: Default::default(),
			statements: Default::default(),
			validators,
			session_index,
			seconded_counts: Default::default(),
			span,
		}
	}

	/// Note the given statement.
	///
	/// If it was not already known and can be accepted,  returns `NotedStatement::Fresh`,
	/// with a handle to the statement.
	///
	/// If it can be accepted, but we already know it, returns `NotedStatement::UsefulButKnown`.
	///
	/// We accept up to `VC_THRESHOLD` (2 at time of writing) `Seconded` statements
	/// per validator. These will be the first ones we see. The statement is assumed
	/// to have been checked, including that the validator index is not out-of-bounds and
	/// the signature is valid.
	///
	/// Any other statements or those that reference a candidate we are not aware of cannot be accepted
	/// and will return `NotedStatement::NotUseful`.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn note_statement(&mut self, statement: SignedFullStatement) -> NotedStatement {
		let validator_index = statement.validator_index();
		let comparator = StoredStatementComparator {
			compact: statement.payload().to_compact(),
			validator_index,
			signature: statement.signature().clone(),
		};

		let stored = StoredStatement {
			comparator: comparator.clone(),
			statement,
		};

		match comparator.compact {
			CompactStatement::Candidate(h) => {
				let seconded_so_far = self.seconded_counts.entry(validator_index).or_insert(0);
				if *seconded_so_far >= VC_THRESHOLD {
					return NotedStatement::NotUseful;
				}

				self.candidates.insert(h);
				if self.statements.insert(stored) {
					*seconded_so_far += 1;

					// This will always return `Some` because it was just inserted.
					NotedStatement::Fresh(self.statements.get(&comparator)
						.expect("Statement was just inserted; qed"))
				} else {
					NotedStatement::UsefulButKnown
				}
			}
			CompactStatement::Valid(h) | CompactStatement::Invalid(h) => {
				if !self.candidates.contains(&h) {
					return NotedStatement::NotUseful;
				}

				if self.statements.insert(stored) {
					// This will always return `Some` because it was just inserted.
					NotedStatement::Fresh(self.statements.get(&comparator)
						.expect("Statement was just inserted; qed"))
				} else {
					NotedStatement::UsefulButKnown
				}
			}
		}
	}

	/// Get an iterator over all statements for the active head. Seconded statements come first.
	fn statements(&self) -> impl Iterator<Item = &'_ StoredStatement> + '_ {
		self.statements.iter()
	}

	/// Get an iterator over all statements for the active head that are for a particular candidate.
	fn statements_about(&self, candidate_hash: CandidateHash)
		-> impl Iterator<Item = &'_ StoredStatement> + '_
	{
		self.statements().filter(move |s| s.compact().candidate_hash() == &candidate_hash)
	}
}

/// Check a statement signature under this parent hash.
fn check_statement_signature(
	head: &ActiveHeadData,
	relay_parent: Hash,
	statement: &SignedFullStatement,
) -> Result<(), ()> {
	let signing_context = SigningContext {
		session_index: head.session_index,
		parent_hash: relay_parent,
	};

	head.validators.get(statement.validator_index().0 as usize)
		.ok_or(())
		.and_then(|v| statement.check_signature(&signing_context, v))
}

/// Places the statement in storage if it is new, and then
/// circulates the statement to all peers who have not seen it yet, and
/// sends all statements dependent on that statement to peers who could previously not receive
/// them but now can.
#[tracing::instrument(level = "trace", skip(peers, ctx, active_heads, metrics), fields(subsystem = LOG_TARGET))]
async fn circulate_statement_and_dependents(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	statement: SignedFullStatement,
	metrics: &Metrics,
) {
	let active_head = match active_heads.get_mut(&relay_parent) {
		Some(res) => res,
		None => return,
	};

	let _span = active_head.span.child_builder("circulate-statement")
		.with_candidate(&statement.payload().candidate_hash())
		.with_stage(jaeger::Stage::StatementDistribution)
		.build();

	// First circulate the statement directly to all peers needing it.
	// The borrow of `active_head` needs to encompass only this (Rust) statement.
	let outputs: Option<(CandidateHash, Vec<PeerId>)> = {
		match active_head.note_statement(statement) {
			NotedStatement::Fresh(stored) => Some((
				*stored.compact().candidate_hash(),
				circulate_statement(peers, ctx, relay_parent, stored).await,
			)),
			_ => None,
		}
	};

	let _span = _span.child("send-to-peers");
	// Now send dependent statements to all peers needing them, if any.
	if let Some((candidate_hash, peers_needing_dependents)) = outputs {
		for peer in peers_needing_dependents {
			if let Some(peer_data) = peers.get_mut(&peer) {
				let _span_loop = _span.child_builder("to-peer")
					.with_peer_id(&peer)
					.build();
				// defensive: the peer data should always be some because the iterator
				// of peers is derived from the set of peers.
				send_statements_about(
					peer,
					peer_data,
					ctx,
					relay_parent,
					candidate_hash,
					&*active_head,
					metrics,
				).await;
			}
		}
	}
}

fn statement_message(relay_parent: Hash, statement: SignedFullStatement)
	-> protocol_v1::ValidationProtocol
{
	protocol_v1::ValidationProtocol::StatementDistribution(
		protocol_v1::StatementDistributionMessage::Statement(relay_parent, statement)
	)
}

/// Circulates a statement to all peers who have not seen it yet, and returns
/// an iterator over peers who need to have dependent statements sent.
#[tracing::instrument(level = "trace", skip(peers, ctx), fields(subsystem = LOG_TARGET))]
async fn circulate_statement(
	peers: &mut HashMap<PeerId, PeerData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	stored: &StoredStatement,
) -> Vec<PeerId> {
	let fingerprint = stored.fingerprint();

	let mut peers_to_send = HashMap::new();

	for (peer, data) in peers.iter_mut() {
		if let Some(new_known) = data.send(&relay_parent, &fingerprint) {
			peers_to_send.insert(peer.clone(), new_known);
		}
	}

	// Send all these peers the initial statement.
	if !peers_to_send.is_empty() {
		let payload = statement_message(relay_parent, stored.statement.clone());
		ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
			peers_to_send.keys().cloned().collect(),
			payload,
		))).await;
	}

	peers_to_send.into_iter().filter_map(|(peer, needs_dependent)| if needs_dependent {
		Some(peer)
	} else {
		None
	}).collect()
}

/// Send all statements about a given candidate hash to a peer.
#[tracing::instrument(level = "trace", skip(peer_data, ctx, active_head, metrics), fields(subsystem = LOG_TARGET))]
async fn send_statements_about(
	peer: PeerId,
	peer_data: &mut PeerData,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	active_head: &ActiveHeadData,
	metrics: &Metrics,
) {
	for statement in active_head.statements_about(candidate_hash) {
		if peer_data.send(&relay_parent, &statement.fingerprint()).is_some() {
			let payload = statement_message(
				relay_parent,
				statement.statement.clone(),
			);

			ctx.send_message(AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(vec![peer.clone()], payload)
			)).await;

			metrics.on_statement_distributed();
		}
	}
}

/// Send all statements at a given relay-parent to a peer.
#[tracing::instrument(level = "trace", skip(peer_data, ctx, active_head, metrics), fields(subsystem = LOG_TARGET))]
async fn send_statements(
	peer: PeerId,
	peer_data: &mut PeerData,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	active_head: &ActiveHeadData,
	metrics: &Metrics,
) {
	for statement in active_head.statements() {
		if peer_data.send(&relay_parent, &statement.fingerprint()).is_some() {
			let payload = statement_message(
				relay_parent,
				statement.statement.clone(),
			);

			ctx.send_message(AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(vec![peer.clone()], payload)
			)).await;

			metrics.on_statement_distributed();
		}
	}
}

async fn report_peer(
	ctx: &mut impl SubsystemContext,
	peer: PeerId,
	rep: Rep,
) {
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep)
	)).await
}

// Handle an incoming wire message. Returns a reference to a newly-stored statement
// if we were not already aware of it, along with the corresponding relay-parent.
//
// This function checks the signature and ensures the statement is compatible with our
// view. It also notifies candidate backing if the statement was previously unknown.
#[tracing::instrument(level = "trace", skip(peer_data, ctx, active_heads, metrics), fields(subsystem = LOG_TARGET))]
async fn handle_incoming_message<'a>(
	peer: PeerId,
	peer_data: &mut PeerData,
	our_view: &View,
	active_heads: &'a mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	message: protocol_v1::StatementDistributionMessage,
	metrics: &Metrics,
) -> Option<(Hash, &'a StoredStatement)> {
	let (relay_parent, statement) = match message {
		protocol_v1::StatementDistributionMessage::Statement(r, s) => (r, s),
	};

	if !our_view.contains(&relay_parent) {
		report_peer(ctx, peer, COST_UNEXPECTED_STATEMENT).await;
		return None;
	}

	let active_head = match active_heads.get_mut(&relay_parent) {
		Some(h) => h,
		None => {
			// This should never be out-of-sync with our view if the view updates
			// correspond to actual `StartWork` messages. So we just log and ignore.
			tracing::warn!(
				requested_relay_parent = %relay_parent,
				"our view out-of-sync with active heads; head not found",
			);
			return None;
		}
	};

	let candidate_hash = statement.payload().candidate_hash();
	let handle_incoming_span = active_head.span.child_builder("handle-incoming")
		.with_candidate(&candidate_hash)
		.with_peer_id(&peer)
		.build();

	// check the signature on the statement.
	if let Err(()) = check_statement_signature(&active_head, relay_parent, &statement) {
		report_peer(ctx, peer, COST_INVALID_SIGNATURE).await;
		return None;
	}

	// Ensure the statement is stored in the peer data.
	//
	// Note that if the peer is sending us something that is not within their view,
	// it will not be kept within their log.
	let fingerprint = (statement.payload().to_compact(), statement.validator_index());
	let max_message_count = active_head.validators.len() * 2;
	match peer_data.receive(&relay_parent, &fingerprint, max_message_count) {
		Err(rep) => {
			report_peer(ctx, peer, rep).await;
			return None;
		}
		Ok(true) => {
			// Send the peer all statements concerning the candidate that we have,
			// since it appears to have just learned about the candidate.
			send_statements_about(
				peer.clone(),
				peer_data,
				ctx,
				relay_parent,
				candidate_hash,
				&*active_head,
				metrics,
			).await;
		}
		Ok(false) => {}
	}

	// Note: `peer_data.receive` already ensures that the statement is not an unbounded equivocation
	// or unpinned to a seconded candidate. So it is safe to place it into the storage.
	match active_head.note_statement(statement) {
		NotedStatement::NotUseful => None,
		NotedStatement::UsefulButKnown => {
			report_peer(ctx, peer, BENEFIT_VALID_STATEMENT).await;
			None
		}
		NotedStatement::Fresh(statement) => {
			report_peer(ctx, peer, BENEFIT_VALID_STATEMENT_FIRST).await;

			let mut _span = handle_incoming_span.child("notify-backing");

			// When we receive a new message from a peer, we forward it to the
			// candidate backing subsystem.
			let message = AllMessages::CandidateBacking(
				CandidateBackingMessage::Statement(relay_parent, statement.statement.clone())
			);
			ctx.send_message(message).await;

			Some((relay_parent, statement))
		}
	}
}

/// Update a peer's view. Sends all newly unlocked statements based on the previous
#[tracing::instrument(level = "trace", skip(peer_data, ctx, active_heads, metrics), fields(subsystem = LOG_TARGET))]
async fn update_peer_view_and_send_unlocked(
	peer: PeerId,
	peer_data: &mut PeerData,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	active_heads: &HashMap<Hash, ActiveHeadData>,
	new_view: View,
	metrics: &Metrics,
) {
	let old_view = std::mem::replace(&mut peer_data.view, new_view);

	// Remove entries for all relay-parents in the old view but not the new.
	for removed in old_view.difference(&peer_data.view) {
		let _ = peer_data.view_knowledge.remove(removed);
	}

	// Add entries for all relay-parents in the new view but not the old.
	// Furthermore, send all statements we have for those relay parents.
	let new_view = peer_data.view.difference(&old_view).copied().collect::<Vec<_>>();
	for new in new_view.iter().copied() {
		peer_data.view_knowledge.insert(new, Default::default());

		if let Some(active_head) = active_heads.get(&new) {
			send_statements(
				peer.clone(),
				peer_data,
				ctx,
				new,
				active_head,
				metrics,
			).await;
		}
	}
}

#[tracing::instrument(level = "trace", skip(peers, active_heads, ctx, metrics), fields(subsystem = LOG_TARGET))]
async fn handle_network_update(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	our_view: &mut OurView,
	update: NetworkBridgeEvent<protocol_v1::StatementDistributionMessage>,
	metrics: &Metrics,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer, _role) => {
			peers.insert(peer, PeerData {
				view: Default::default(),
				view_knowledge: Default::default(),
			});
		}
		NetworkBridgeEvent::PeerDisconnected(peer) => {
			peers.remove(&peer);
		}
		NetworkBridgeEvent::PeerMessage(peer, message) => {
			let handled_incoming = match peers.get_mut(&peer) {
				Some(data) => {
					handle_incoming_message(
						peer,
						data,
						&*our_view,
						active_heads,
						ctx,
						message,
						metrics,
					).await
				}
				None => None,
			};

			// if we got a fresh message, we need to circulate it to all peers.
			if let Some((relay_parent, statement)) = handled_incoming {
				// we can ignore the set of peers who this function returns as now expecting
				// dependent statements.
				//
				// we have the invariant in this subsystem that we never store a `Valid` or `Invalid`
				// statement before a `Seconded` statement. `Seconded` statements are the only ones
				// that require dependents. Thus, if this is a `Seconded` statement for a candidate we
				// were not aware of before, we cannot have any dependent statements from the candidate.
				let _ = circulate_statement(
					peers,
					ctx,
					relay_parent,
					statement,
				).await;
			}
		}
		NetworkBridgeEvent::PeerViewChange(peer, view) => {
			match peers.get_mut(&peer) {
				Some(data) => {
					update_peer_view_and_send_unlocked(
						peer,
						data,
						ctx,
						&*active_heads,
						view,
						metrics,
					).await
				}
				None => (),
			}
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			let old_view = std::mem::replace(our_view, view);
			active_heads.retain(|head, _| our_view.contains(head));

			for new in our_view.difference(&old_view) {
				if !active_heads.contains_key(&new) {
					tracing::warn!(
						target: LOG_TARGET,
						unknown_hash = %new,
						"Our network bridge view update \
						inconsistent with `StartWork` messages we have received from overseer. \
						Contains unknown hash.",
					);
				}
			}
		}
	}

}

impl StatementDistribution {
	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run(
		self,
		mut ctx: impl SubsystemContext<Message = StatementDistributionMessage>,
	) -> SubsystemResult<()> {
		let mut peers: HashMap<PeerId, PeerData> = HashMap::new();
		let mut our_view = OurView::default();
		let mut active_heads: HashMap<Hash, ActiveHeadData> = HashMap::new();
		let metrics = self.metrics;

		loop {
			let message = ctx.recv().await?;
			match message {
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, .. })) => {
					let _timer = metrics.time_active_leaves_update();

					for (relay_parent, span) in activated {
						let span = PerLeafSpan::new(span, "statement-distribution");

						let (validators, session_index) = {
							let (val_tx, val_rx) = oneshot::channel();
							let (session_tx, session_rx) = oneshot::channel();

							let val_message = AllMessages::RuntimeApi(
								RuntimeApiMessage::Request(
									relay_parent,
									RuntimeApiRequest::Validators(val_tx),
								),
							);
							let session_message = AllMessages::RuntimeApi(
								RuntimeApiMessage::Request(
									relay_parent,
									RuntimeApiRequest::SessionIndexForChild(session_tx),
								),
							);

							ctx.send_messages(
								std::iter::once(val_message).chain(std::iter::once(session_message))
							).await;

							match (val_rx.await?, session_rx.await?) {
								(Ok(v), Ok(s)) => (v, s),
								(Err(e), _) | (_, Err(e)) => {
									tracing::warn!(
										target: LOG_TARGET,
										err = ?e,
										"Failed to fetch runtime API data for active leaf",
									);

									// Lacking this bookkeeping might make us behave funny, although
									// not in any slashable way. But we shouldn't take down the node
									// on what are likely spurious runtime API errors.
									continue;
								}
							}
						};

						active_heads.entry(relay_parent)
							.or_insert(ActiveHeadData::new(validators, session_index, span));
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {
					// do nothing
				}
				FromOverseer::Signal(OverseerSignal::Conclude) => break,
				FromOverseer::Communication { msg } => match msg {
					StatementDistributionMessage::Share(relay_parent, statement) => {
						let _timer = metrics.time_share();

						circulate_statement_and_dependents(
							&mut peers,
							&mut active_heads,
							&mut ctx,
							relay_parent,
							statement,
							&metrics,
						).await;
					}
					StatementDistributionMessage::NetworkBridgeUpdateV1(event) => {
						let _timer = metrics.time_network_bridge_update_v1();

						handle_network_update(
							&mut peers,
							&mut active_heads,
							&mut ctx,
							&mut our_view,
							event,
							&metrics,
						).await;
					}
				}
			}
		}
		Ok(())
	}
}

#[derive(Clone)]
struct MetricsInner {
	statements_distributed: prometheus::Counter<prometheus::U64>,
	active_leaves_update: prometheus::Histogram,
	share: prometheus::Histogram,
	network_bridge_update_v1: prometheus::Histogram,
}

/// Statement Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_statement_distributed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.statements_distributed.inc();
		}
	}

	/// Provide a timer for `active_leaves_update` which observes on drop.
	fn time_active_leaves_update(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.active_leaves_update.start_timer())
	}

	/// Provide a timer for `share` which observes on drop.
	fn time_share(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.share.start_timer())
	}

	/// Provide a timer for `network_bridge_update_v1` which observes on drop.
	fn time_network_bridge_update_v1(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.network_bridge_update_v1.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			statements_distributed: prometheus::register(
				prometheus::Counter::new(
					"parachain_statements_distributed_total",
					"Number of candidate validity statements distributed to other peers."
				)?,
				registry,
			)?,
			active_leaves_update: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_statement_distribution_active_leaves_update",
						"Time spent within `statement_distribution::active_leaves_update`",
					)
				)?,
				registry,
			)?,
			share: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_statement_distribution_share",
						"Time spent within `statement_distribution::share`",
					)
				)?,
				registry,
			)?,
			network_bridge_update_v1: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_statement_distribution_network_bridge_update_v1",
						"Time spent within `statement_distribution::network_bridge_update_v1`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Arc;
	use sp_keyring::Sr25519Keyring;
	use sp_application_crypto::AppKey;
	use polkadot_node_primitives::Statement;
	use polkadot_primitives::v1::CommittedCandidateReceipt;
	use assert_matches::assert_matches;
	use futures::executor::{self, block_on};
	use sp_keystore::{CryptoStore, SyncCryptoStorePtr, SyncCryptoStore};
	use sc_keystore::LocalKeystore;
	use polkadot_node_network_protocol::{view, ObservedRole, our_view};
	use polkadot_subsystem::jaeger;

	#[test]
	fn active_head_accepts_only_2_seconded_per_validator() {
		let validators = vec![
			Sr25519Keyring::Alice.public().into(),
			Sr25519Keyring::Bob.public().into(),
			Sr25519Keyring::Charlie.public().into(),
		];
		let parent_hash: Hash = [1; 32].into();

		let session_index = 1;
		let signing_context = SigningContext {
			parent_hash,
			session_index,
		};

		let candidate_a = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = 1.into();
			c
		};

		let candidate_b = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = 2.into();
			c
		};

		let candidate_c = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = 3.into();
			c
		};

		let mut head_data = ActiveHeadData::new(
			validators,
			session_index,
			PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
		);

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
		let alice_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Alice.to_seed())
		).unwrap();
		let bob_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Bob.to_seed())
		).unwrap();

		// note A
		let a_seconded_val_0 = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_a.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)).ok().flatten().expect("should be signed");
		let noted = head_data.note_statement(a_seconded_val_0.clone());

		assert_matches!(noted, NotedStatement::Fresh(_));

		// note A (duplicate)
		let noted = head_data.note_statement(a_seconded_val_0);

		assert_matches!(noted, NotedStatement::UsefulButKnown);

		// note B
		let noted = head_data.note_statement(block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_b.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)).ok().flatten().expect("should be signed"));

		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (beyond 2 - ignored)
		let noted = head_data.note_statement(block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)).ok().flatten().expect("should be signed"));

		assert_matches!(noted, NotedStatement::NotUseful);

		// note B (new validator)
		let noted = head_data.note_statement(block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_b.clone()),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		)).ok().flatten().expect("should be signed"));

		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (new validator)
		let noted = head_data.note_statement(block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		)).ok().flatten().expect("should be signed"));

		assert_matches!(noted, NotedStatement::Fresh(_));
	}

	#[test]
	fn note_local_works() {
		let hash_a = CandidateHash([1; 32].into());
		let hash_b = CandidateHash([2; 32].into());

		let mut per_peer_tracker = VcPerPeerTracker::default();
		per_peer_tracker.note_local(hash_a.clone());
		per_peer_tracker.note_local(hash_b.clone());

		assert!(per_peer_tracker.local_observed.contains(&hash_a));
		assert!(per_peer_tracker.local_observed.contains(&hash_b));

		assert!(!per_peer_tracker.remote_observed.contains(&hash_a));
		assert!(!per_peer_tracker.remote_observed.contains(&hash_b));
	}

	#[test]
	fn note_remote_works() {
		let hash_a = CandidateHash([1; 32].into());
		let hash_b = CandidateHash([2; 32].into());
		let hash_c = CandidateHash([3; 32].into());

		let mut per_peer_tracker = VcPerPeerTracker::default();
		assert!(per_peer_tracker.note_remote(hash_a.clone()));
		assert!(per_peer_tracker.note_remote(hash_b.clone()));
		assert!(!per_peer_tracker.note_remote(hash_c.clone()));

		assert!(per_peer_tracker.remote_observed.contains(&hash_a));
		assert!(per_peer_tracker.remote_observed.contains(&hash_b));
		assert!(!per_peer_tracker.remote_observed.contains(&hash_c));

		assert!(!per_peer_tracker.local_observed.contains(&hash_a));
		assert!(!per_peer_tracker.local_observed.contains(&hash_b));
		assert!(!per_peer_tracker.local_observed.contains(&hash_c));
	}

	#[test]
	fn per_peer_relay_parent_knowledge_send() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());

		// Sending an un-pinned statement should not work and should have no effect.
		assert!(knowledge.send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))).is_none());
		assert!(!knowledge.known_candidates.contains(&hash_a));
		assert!(knowledge.sent_statements.is_empty());
		assert!(knowledge.received_statements.is_empty());
		assert!(knowledge.seconded_counts.is_empty());
		assert!(knowledge.received_message_count.is_empty());

		// Make the peer aware of the candidate.
		assert_eq!(knowledge.send(&(CompactStatement::Candidate(hash_a), ValidatorIndex(0))), Some(true));
		assert_eq!(knowledge.send(&(CompactStatement::Candidate(hash_a), ValidatorIndex(1))), Some(false));
		assert!(knowledge.known_candidates.contains(&hash_a));
		assert_eq!(knowledge.sent_statements.len(), 2);
		assert!(knowledge.received_statements.is_empty());
		assert_eq!(knowledge.seconded_counts.len(), 2);
		assert!(knowledge.received_message_count.get(&hash_a).is_none());

		// And now it should accept the dependent message.
		assert_eq!(knowledge.send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))), Some(false));
		assert!(knowledge.known_candidates.contains(&hash_a));
		assert_eq!(knowledge.sent_statements.len(), 3);
		assert!(knowledge.received_statements.is_empty());
		assert_eq!(knowledge.seconded_counts.len(), 2);
		assert!(knowledge.received_message_count.get(&hash_a).is_none());
	}

	#[test]
	fn cant_send_after_receiving() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());
		assert!(knowledge.receive(&(CompactStatement::Candidate(hash_a), ValidatorIndex(0)), 3).unwrap());
		assert!(knowledge.send(&(CompactStatement::Candidate(hash_a), ValidatorIndex(0))).is_none());
	}

	#[test]
	fn per_peer_relay_parent_knowledge_receive() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());

		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);

		assert_eq!(
			knowledge.receive(&(CompactStatement::Candidate(hash_a), ValidatorIndex(0)), 3),
			Ok(true),
		);

		// Push statements up to the flood limit.
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3),
			Ok(false),
		);

		assert!(knowledge.known_candidates.contains(&hash_a));
		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 2);

		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Ok(false),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);

		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
			Err(COST_APPARENT_FLOOD),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);
		assert_eq!(knowledge.received_statements.len(), 3); // number of prior `Ok`s.

		// Now make sure that the seconding limit is respected.
		let hash_b = CandidateHash([2; 32].into());
		let hash_c = CandidateHash([3; 32].into());

		assert_eq!(
			knowledge.receive(&(CompactStatement::Candidate(hash_b), ValidatorIndex(0)), 3),
			Ok(true),
		);

		assert_eq!(
			knowledge.receive(&(CompactStatement::Candidate(hash_c), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);

		// Last, make sure that already-known statements are disregarded.
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Err(COST_DUPLICATE_STATEMENT),
		);

		assert_eq!(
			knowledge.receive(&(CompactStatement::Candidate(hash_b), ValidatorIndex(0)), 3),
			Err(COST_DUPLICATE_STATEMENT),
		);
	}

	#[test]
	fn peer_view_update_sends_messages() {
		let hash_a = Hash::repeat_byte(1);
		let hash_b = Hash::repeat_byte(2);
		let hash_c = Hash::repeat_byte(3);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_c;
			c.descriptor.para_id = 1.into();
			c
		};
		let candidate_hash = candidate.hash();

		let old_view = view![hash_a, hash_b];
		let new_view = view![hash_b, hash_c];

		let mut active_heads = HashMap::new();
		let validators = vec![
			Sr25519Keyring::Alice.public().into(),
			Sr25519Keyring::Bob.public().into(),
			Sr25519Keyring::Charlie.public().into(),
		];

		let session_index = 1;
		let signing_context = SigningContext {
			parent_hash: hash_c,
			session_index,
		};

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());

		let alice_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Alice.to_seed())
		).unwrap();
		let bob_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Bob.to_seed())
		).unwrap();
		let charlie_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Charlie.to_seed())
		).unwrap();

		let new_head_data = {
			let mut data = ActiveHeadData::new(
				validators,
				session_index,
				PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
			);

			let noted = data.note_statement(block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)).ok().flatten().expect("should be signed"));

			assert_matches!(noted, NotedStatement::Fresh(_));

			let noted = data.note_statement(block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Valid(candidate_hash),
				&signing_context,
				ValidatorIndex(1),
				&bob_public.into(),
			)).ok().flatten().expect("should be signed"));

			assert_matches!(noted, NotedStatement::Fresh(_));

			let noted = data.note_statement(block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Valid(candidate_hash),
				&signing_context,
				ValidatorIndex(2),
				&charlie_public.into(),
			)).ok().flatten().expect("should be signed"));

			assert_matches!(noted, NotedStatement::Fresh(_));

			data
		};

		active_heads.insert(hash_c, new_head_data);

		let mut peer_data = PeerData {
			view: old_view,
			view_knowledge: {
				let mut k = HashMap::new();

				k.insert(hash_a, Default::default());
				k.insert(hash_b, Default::default());

				k
			},
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
		let peer = PeerId::random();

		executor::block_on(async move {
			update_peer_view_and_send_unlocked(
				peer.clone(),
				&mut peer_data,
				&mut ctx,
				&active_heads,
				new_view.clone(),
				&Default::default(),
			).await;

			assert_eq!(peer_data.view, new_view);
			assert!(!peer_data.view_knowledge.contains_key(&hash_a));
			assert!(peer_data.view_knowledge.contains_key(&hash_b));

			let c_knowledge = peer_data.view_knowledge.get(&hash_c).unwrap();

			assert!(c_knowledge.known_candidates.contains(&candidate_hash));
			assert!(c_knowledge.sent_statements.contains(
				&(CompactStatement::Candidate(candidate_hash), ValidatorIndex(0))
			));
			assert!(c_knowledge.sent_statements.contains(
				&(CompactStatement::Valid(candidate_hash), ValidatorIndex(1))
			));
			assert!(c_knowledge.sent_statements.contains(
				&(CompactStatement::Valid(candidate_hash), ValidatorIndex(2))
			));

			// now see if we got the 3 messages from the active head data.
			let active_head = active_heads.get(&hash_c).unwrap();

			// semi-fragile because hashmap iterator ordering is undefined, but in practice
			// it will not change between runs of the program.
			for statement in active_head.statements_about(candidate_hash) {
				let message = handle.recv().await;
				let expected_to = vec![peer.clone()];
				let expected_payload
					= statement_message(hash_c, statement.statement.clone());

				assert_matches!(
					message,
					AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
						to,
						payload,
					)) => {
						assert_eq!(to, expected_to);
						assert_eq!(payload, expected_payload)
					}
				)
			}
		});
	}

	#[test]
	fn circulated_statement_goes_to_all_peers_with_view() {
		let hash_a = Hash::repeat_byte(1);
		let hash_b = Hash::repeat_byte(2);
		let hash_c = Hash::repeat_byte(3);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_b;
			c.descriptor.para_id = 1.into();
			c
		};

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		let peer_a_view = view![hash_a];
		let peer_b_view = view![hash_a, hash_b];
		let peer_c_view = view![hash_b, hash_c];

		let session_index = 1;

		let peer_data_from_view = |view: View| PeerData {
			view: view.clone(),
			view_knowledge: view.iter().map(|v| (v.clone(), Default::default())).collect(),
		};

		let mut peer_data: HashMap<_, _> = vec![
			(peer_a.clone(), peer_data_from_view(peer_a_view)),
			(peer_b.clone(), peer_data_from_view(peer_b_view)),
			(peer_c.clone(), peer_data_from_view(peer_c_view)),
		].into_iter().collect();

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		executor::block_on(async move {
			let statement = {
				let signing_context = SigningContext {
					parent_hash: hash_b,
					session_index,
				};

				let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
				let alice_public = CryptoStore::sr25519_generate_new(
					&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Alice.to_seed())
				).await.unwrap();

				let statement = SignedFullStatement::sign(
					&keystore,
					Statement::Seconded(candidate),
					&signing_context,
					ValidatorIndex(0),
					&alice_public.into(),
				).await.ok().flatten().expect("should be signed");

				StoredStatement {
					comparator: StoredStatementComparator {
						compact: statement.payload().to_compact(),
						validator_index: ValidatorIndex(0),
						signature: statement.signature().clone()
					},
					statement,
				}
			};

			let needs_dependents = circulate_statement(
				&mut peer_data,
				&mut ctx,
				hash_b,
				&statement,
			).await;

			{
				assert_eq!(needs_dependents.len(), 2);
				assert!(needs_dependents.contains(&peer_b));
				assert!(needs_dependents.contains(&peer_c));
			}

			let fingerprint = (statement.compact().clone(), ValidatorIndex(0));

			assert!(
				peer_data.get(&peer_b).unwrap()
				.view_knowledge.get(&hash_b).unwrap()
				.sent_statements.contains(&fingerprint),
			);

			assert!(
				peer_data.get(&peer_c).unwrap()
				.view_knowledge.get(&hash_b).unwrap()
				.sent_statements.contains(&fingerprint),
			);

			let message = handle.recv().await;
			assert_matches!(
				message,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
					to,
					payload,
				)) => {
					assert_eq!(to.len(), 2);
					assert!(to.contains(&peer_b));
					assert!(to.contains(&peer_c));

					assert_eq!(
						payload,
						statement_message(hash_b, statement.statement.clone()),
					);
				}
			)
		});
	}

	#[test]
	fn receiving_from_one_sends_to_another_and_to_candidate_backing() {
		let hash_a = Hash::repeat_byte(1);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_a;
			c.descriptor.para_id = 1.into();
			c
		};

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let validators = vec![
			Sr25519Keyring::Alice.public().into(),
			Sr25519Keyring::Bob.public().into(),
			Sr25519Keyring::Charlie.public().into(),
		];

		let session_index = 1;

		let pool = sp_core::testing::TaskExecutor::new();
		let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bg = async move {
			let s = StatementDistribution { metrics: Default::default() };
			s.run(ctx).await.unwrap();
		};

		let test_fut = async move {
			// register our active heads.
			handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![(hash_a, Arc::new(jaeger::Span::Disabled))].into(),
				deactivated: vec![].into(),
			}))).await;

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::Validators(tx))
				)
					if r == hash_a
				=> {
					let _ = tx.send(Ok(validators));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
				)
					if r == hash_a
				=> {
					let _ = tx.send(Ok(session_index));
				}
			);

			// notify of peers and view
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_a.clone(), ObservedRole::Full)
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full)
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a])
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a])
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_a])
				)
			}).await;

			// receive a seconded statement from peer A. it should be propagated onwards to peer B and to
			// candidate backing.
			let statement = {
				let signing_context = SigningContext {
					parent_hash: hash_a,
					session_index,
				};

				let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
				let alice_public = CryptoStore::sr25519_generate_new(
					&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Alice.to_seed())
				).await.unwrap();

				SignedFullStatement::sign(
					&keystore,
					Statement::Seconded(candidate),
					&signing_context,
					ValidatorIndex(0),
					&alice_public.into(),
				).await.ok().flatten().expect("should be signed")
			};

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						protocol_v1::StatementDistributionMessage::Statement(hash_a, statement.clone()),
					)
				)
			}).await;

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(p, r)
				) if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => {}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::CandidateBacking(
					CandidateBackingMessage::Statement(r, s)
				) if r == hash_a && s == statement => {}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						recipients,
						protocol_v1::ValidationProtocol::StatementDistribution(
							protocol_v1::StatementDistributionMessage::Statement(r, s)
						),
					)
				) => {
					assert_eq!(recipients, vec![peer_b.clone()]);
					assert_eq!(r, hash_a);
					assert_eq!(s, statement);
				}
			);
		};

		futures::pin_mut!(test_fut);
		futures::pin_mut!(bg);

		executor::block_on(future::select(test_fut, bg));
	}
}
