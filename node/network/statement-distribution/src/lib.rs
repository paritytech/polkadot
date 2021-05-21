// Copyright 2021 Parity Technologies (UK) Ltd.
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

use error::{FatalResult, NonFatalResult, log_error};
use parity_scale_codec::Encode;

use polkadot_subsystem::{
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, PerLeafSpan, SpawnedSubsystem, Subsystem,
	SubsystemContext, SubsystemError, jaeger,
	messages::{
		AllMessages, NetworkBridgeMessage, StatementDistributionMessage,
		CandidateBackingMessage, NetworkBridgeEvent,
	},
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
	self as util, MIN_GOSSIP_PEERS,
};
use polkadot_node_primitives::{SignedFullStatement, UncheckedSignedFullStatement, Statement};
use polkadot_primitives::v1::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash,
	SigningContext, ValidatorId, ValidatorIndex, ValidatorSignature, AuthorityDiscoveryId,
};
use polkadot_node_network_protocol::{
	IfDisconnected, PeerId, UnifiedReputationChange as Rep, View,
	peer_set::{
		IsAuthority, PeerSet
	},
	v1::{
		self as protocol_v1, StatementMetadata
	}
};

use futures::{channel::mpsc, future::RemoteHandle, prelude::*};
use futures::channel::oneshot;
use indexmap::{IndexMap, map::Entry as IEntry};
use sp_keystore::SyncCryptoStorePtr;
use util::{Fault, runtime::RuntimeInfo};

use std::collections::{HashMap, HashSet, hash_map::Entry};

mod error;
pub use error::{Error, NonFatal, Fatal, Result};

/// Background task logic for requesting of large statements.
mod requester;
use requester::{RequesterMessage, fetch};

/// Background task logic for responding for large statements.
mod responder;
use responder::{ResponderMessage, respond};

const COST_UNEXPECTED_STATEMENT: Rep = Rep::CostMinor("Unexpected Statement");
const COST_FETCH_FAIL: Rep = Rep::CostMinor("Requesting `CommittedCandidateReceipt` from peer failed");
const COST_INVALID_SIGNATURE: Rep = Rep::CostMajor("Invalid Statement Signature");
const COST_WRONG_HASH: Rep = Rep::CostMajor("Received candidate had wrong hash");
const COST_DUPLICATE_STATEMENT: Rep = Rep::CostMajorRepeated("Statement sent more than once by peer");
const COST_APPARENT_FLOOD: Rep = Rep::Malicious("Peer appears to be flooding us with statements");

const BENEFIT_VALID_STATEMENT: Rep = Rep::BenefitMajor("Peer provided a valid statement");
const BENEFIT_VALID_STATEMENT_FIRST: Rep = Rep::BenefitMajorFirst(
	"Peer was the first to provide a valid statement",
);
const BENEFIT_VALID_RESPONSE: Rep = Rep::BenefitMajor("Peer provided a valid large statement response");

/// The maximum amount of candidates each validator is allowed to second at any relay-parent.
/// Short for "Validator Candidate Threshold".
///
/// This is the amount of candidates we keep per validator at any relay-parent.
/// Typically we will only keep 1, but when a validator equivocates we will need to track 2.
const VC_THRESHOLD: usize = 2;

const LOG_TARGET: &str = "parachain::statement-distribution";

/// Large statements should be rare.
const MAX_LARGE_STATEMENTS_PER_SENDER: usize = 20;

/// The statement distribution subsystem.
pub struct StatementDistribution {
	/// Pointer to a keystore, which is required for determining this nodes validator index.
	keystore: SyncCryptoStorePtr,
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
			future: self
				.run(ctx)
				.map_err(|e| SubsystemError::with_origin("statement-distribution", e))
				.boxed(),
		}
	}
}

impl StatementDistribution {
	/// Create a new Statement Distribution Subsystem
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> StatementDistribution {
		StatementDistribution {
			keystore,
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
			tracing::warn!(
				target: LOG_TARGET,
				"Statement distribution is erroneously attempting to distribute more \
				than {} candidate(s) per validator index. Ignoring",
				VC_THRESHOLD,
			);
		}
	}

	/// Note that the remote should now be aware that a validator has seconded a given candidate (by hash)
	/// based on a message that it has sent us.
	///
	/// Returns `true` if the peer was allowed to send us such a message, `false` otherwise.
	fn note_remote(&mut self, h: CandidateHash) -> bool {
		note_hash(&mut self.remote_observed, h)
	}

	/// Returns `true` if the peer is allowed to send us such a message, `false` otherwise.
	fn is_wanted_candidate(&self, h: &CandidateHash) -> bool {
		!self.remote_observed.contains(h) &&
		!self.remote_observed.is_full()
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
	/// candidates that the peer is aware of because we sent statements to it. This indicates that we can
	/// send other statements pertaining to that candidate.
	sent_candidates: HashSet<CandidateHash>,
	/// candidates that peer is aware of, because we received statements from it.
	received_candidates: HashSet<CandidateHash>,
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


	/// How many large statements this peer already sent us.
	///
	/// Flood protection for large statements is rather hard and as soon as we get
	/// https://github.com/paritytech/polkadot/issues/2979 implemented also no longer necessary.
	/// Reason: We keep messages around until we fetched the payload, but if a node makes up
	/// statements and never provides the data, we will keep it around for the slot duration. Not
	/// even signature checking would help, as the sender, if a validator, can just sign arbitrary
	/// invalid statements and will not face any consequences as long as it won't provide the
	/// payload.
	///
	/// Quick and temporary fix, only accept `MAX_LARGE_STATEMENTS_PER_SENDER` per connected node.
	///
	/// Large statements should be rare, if they were not, we would run into problems anyways, as
	/// we would not be able to distribute them in a timely manner. Therefore
	/// `MAX_LARGE_STATEMENTS_PER_SENDER` can be set to a relatively small number. It is also not
	/// per candidate hash, but in total as candidate hashes can be made up, as illustrated above.
	///
	/// An attacker could still try to fill up our memory, by repeatedly disconnecting and
	/// connecting again with new peer ids, but we assume that the resulting effective bandwidth
	/// for such an attack would be too low.
	large_statement_count: usize,
}

impl PeerRelayParentKnowledge {
	/// Updates our view of the peer's knowledge with this statement's fingerprint based
	/// on something that we would like to send to the peer.
	///
	/// NOTE: assumes `self.can_send` returned true before this call.
	///
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	///
	/// This returns `true` if this is the first time the peer has become aware of a
	/// candidate with the given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn send(&mut self, fingerprint: &(CompactStatement, ValidatorIndex)) -> bool {
		debug_assert!(
			self.can_send(fingerprint),
			"send is only called after `can_send` returns true; qed",
		);

		let new_known = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				self.seconded_counts.entry(fingerprint.1)
					.or_default()
					.note_local(h.clone());

				self.sent_candidates.insert(h.clone())
			},
			CompactStatement::Valid(_) => {
				false
			}
		};

		self.sent_statements.insert(fingerprint.clone());

		new_known
	}

	/// This returns `true` if the peer cannot accept this statement, without altering internal
	/// state, `false` otherwise.
	fn can_send(&self, fingerprint: &(CompactStatement, ValidatorIndex)) -> bool {
		let already_known = self.sent_statements.contains(fingerprint)
			|| self.received_statements.contains(fingerprint);

		if already_known {
			return false;
		}

		match fingerprint.0 {
			CompactStatement::Valid(ref h) => {
				// The peer can only accept Valid and Invalid statements for which it is aware
				// of the corresponding candidate.
				self.is_known_candidate(h)
			}
			CompactStatement::Seconded(_) => {
				true
			},
		}
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
	) -> std::result::Result<bool, Rep> {
		// We don't check `sent_statements` because a statement could be in-flight from both
		// sides at the same time.
		if self.received_statements.contains(fingerprint) {
			return Err(COST_DUPLICATE_STATEMENT);
		}

		let candidate_hash = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				let allowed_remote = self.seconded_counts.entry(fingerprint.1)
					.or_insert_with(Default::default)
					.note_remote(h.clone());

				if !allowed_remote {
					return Err(COST_UNEXPECTED_STATEMENT);
				}

				h
			}
			CompactStatement::Valid(ref h) => {
				if !self.is_known_candidate(&h) {
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
		Ok(self.received_candidates.insert(candidate_hash.clone()))
	}

	/// Note a received large statement metadata.
	fn receive_large_statement(&mut self) -> std::result::Result<(), Rep> {
		if self.large_statement_count >= MAX_LARGE_STATEMENTS_PER_SENDER {
			return Err(COST_APPARENT_FLOOD);
		}
		self.large_statement_count += 1;
		Ok(())
	}

	/// This method does the same checks as `receive` without modifying the internal state.
	/// Returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	fn check_can_receive(
		&self,
		fingerprint: &(CompactStatement, ValidatorIndex),
		max_message_count: usize,
	) -> std::result::Result<(), Rep> {
		// We don't check `sent_statements` because a statement could be in-flight from both
		// sides at the same time.
		if self.received_statements.contains(fingerprint) {
			return Err(COST_DUPLICATE_STATEMENT);
		}

		let candidate_hash = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				let allowed_remote = self.seconded_counts.get(&fingerprint.1)
					.map_or(true, |r| r.is_wanted_candidate(h));

				if !allowed_remote {
					return Err(COST_UNEXPECTED_STATEMENT);
				}

				h
			}
			CompactStatement::Valid(ref h) => {
				if !self.is_known_candidate(&h) {
					return Err(COST_UNEXPECTED_STATEMENT);
				}

				h
			}
		};

		let received_per_candidate = self.received_message_count
			.get(candidate_hash)
			.unwrap_or(&0);

		if *received_per_candidate >= max_message_count {
			Err(COST_APPARENT_FLOOD)
		} else {
			Ok(())
		}
	}

	/// Check for candidates that the peer is aware of. This indicates that we can
	/// send other statements pertaining to that candidate.
	fn is_known_candidate(&self, candidate: &CandidateHash) -> bool {
		self.sent_candidates.contains(candidate) || self.received_candidates.contains(candidate)
	}
}

struct PeerData {
	view: View,
	view_knowledge: HashMap<Hash, PeerRelayParentKnowledge>,
	// Peer might be an authority.
	maybe_authority: Option<AuthorityDiscoveryId>,
}

impl PeerData {
	/// Updates our view of the peer's knowledge with this statement's fingerprint based
	/// on something that we would like to send to the peer.
	///
	/// NOTE: assumes `self.can_send` returned true before this call.
	///
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	///
	/// This returns `true` if this is the first time the peer has become aware of a
	/// candidate with the given hash.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn send(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
	) -> bool {
		debug_assert!(
			self.can_send(relay_parent, fingerprint),
			"send is only called after `can_send` returns true; qed",
		);
		self.view_knowledge
			.get_mut(relay_parent)
			.expect("send is only called after `can_send` returns true; qed")
			.send(fingerprint)
	}

	/// This returns `None` if the peer cannot accept this statement, without altering internal
	/// state.
	fn can_send(
		&self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
	) -> bool {
		self.view_knowledge
			.get(relay_parent)
			.map_or(false, |k| k.can_send(fingerprint))
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
	) -> std::result::Result<bool, Rep> {
		self.view_knowledge
			.get_mut(relay_parent)
			.ok_or(COST_UNEXPECTED_STATEMENT)?
			.receive(fingerprint, max_message_count)
	}

	/// This method does the same checks as `receive` without modifying the internal state.
	/// Returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	fn check_can_receive(
		&self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
		max_message_count: usize,
	) -> std::result::Result<(), Rep> {
		self.view_knowledge
			.get(relay_parent)
			.ok_or(COST_UNEXPECTED_STATEMENT)?
			.check_can_receive(fingerprint, max_message_count)
	}

	/// Basic flood protection for large statements.
	fn receive_large_statement(
		&mut self,
		relay_parent: &Hash,
	) -> std::result::Result<(), Rep> {
		self.view_knowledge
			.get_mut(relay_parent)
			.ok_or(COST_UNEXPECTED_STATEMENT)?
			.receive_large_statement()
	}
}

// A statement stored while a relay chain head is active.
#[derive(Debug, Copy, Clone)]
struct StoredStatement<'a> {
	comparator: &'a StoredStatementComparator,
	statement: &'a SignedFullStatement,
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

impl<'a> From<(&'a StoredStatementComparator, &'a SignedFullStatement)> for StoredStatement<'a> {
	fn from((comparator, statement): (&'a StoredStatementComparator, &'a SignedFullStatement)) -> Self {
		Self { comparator, statement }
	}
}

impl<'a> StoredStatement<'a> {
	fn compact(&self) -> &'a CompactStatement {
		&self.comparator.compact
	}

	fn fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
		(self.comparator.compact.clone(), self.statement.validator_index())
	}
}

#[derive(Debug)]
enum NotedStatement<'a> {
	NotUseful,
	Fresh(StoredStatement<'a>),
	UsefulButKnown
}

/// Large statement fetching status.
enum LargeStatementStatus {
	/// We are currently fetching the statement data from a remote peer. We keep a list of other nodes
	/// claiming to have that data and will fallback on them.
	Fetching(FetchingInfo),
	/// Statement data is fetched or we got it locally via `StatementDistributionMessage::Share`.
	FetchedOrShared(CommittedCandidateReceipt),
}

/// Info about a fetch in progress.
struct FetchingInfo {
	/// All peers that send us a `LargeStatement` or a `Valid` statement for the given
	/// `CandidateHash`, together with their originally sent messages.
	///
	/// We use an `IndexMap` here to preserve the ordering of peers sending us messages. This is
	/// desirable because we reward first sending peers with reputation.
	available_peers: IndexMap<PeerId, Vec<protocol_v1::StatementDistributionMessage>>,
	/// Peers left to try in case the background task needs it.
	peers_to_try: Vec<PeerId>,
	/// Sender for sending fresh peers to the fetching task in case of failure.
	peer_sender: Option<oneshot::Sender<Vec<PeerId>>>,
	/// Task taking care of the request.
	///
	/// Will be killed once dropped.
	#[allow(dead_code)]
	fetching_task: RemoteHandle<()>,
}

/// Messages to be handled in this subsystem.
enum Message {
	/// Messages from other subsystems.
	Subsystem(FatalResult<FromOverseer<StatementDistributionMessage>>),
	/// Messages from spawned requester background tasks.
	Requester(Option<RequesterMessage>),
	/// Messages from spawned responder background task.
	Responder(Option<ResponderMessage>)
}

impl Message {
	async fn receive(
		ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
		from_requester: &mut mpsc::Receiver<RequesterMessage>,
		from_responder: &mut mpsc::Receiver<ResponderMessage>,
	) -> Message {
		// We are only fusing here to make `select` happy, in reality we will quit if one of those
		// streams end:
		let from_overseer = ctx.recv().fuse();
		let from_requester = from_requester.next().fuse();
		let from_responder = from_responder.next().fuse();
		futures::pin_mut!(from_overseer, from_requester, from_responder);
		futures::select!(
			msg = from_overseer => Message::Subsystem(msg.map_err(Fatal::SubsystemReceive)),
			msg = from_requester => Message::Requester(msg),
			msg = from_responder => Message::Responder(msg),
		)
	}
}

#[derive(Debug, PartialEq, Eq)]
enum DeniedStatement {
	NotUseful,
	UsefulButKnown,
}

struct ActiveHeadData {
	/// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<CandidateHash>,
	/// Stored statements for circulation to peers.
	///
	/// These are iterable in insertion order, and `Seconded` statements are always
	/// accepted before dependent statements.
	statements: IndexMap<StoredStatementComparator, SignedFullStatement>,
	/// Large statements we are waiting for with associated meta data.
	waiting_large_statements: HashMap<CandidateHash, LargeStatementStatus>,
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
			waiting_large_statements: Default::default(),
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

		match comparator.compact {
			CompactStatement::Seconded(h) => {
				let seconded_so_far = self.seconded_counts.entry(validator_index).or_insert(0);
				if *seconded_so_far >= VC_THRESHOLD {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Extra statement is ignored"
					);
					return NotedStatement::NotUseful;
				}

				self.candidates.insert(h);
				if let Some(old) = self.statements.insert(comparator.clone(), statement) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?old,
						"Known statement"
					);
					NotedStatement::UsefulButKnown
				} else {
					*seconded_so_far += 1;

					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?self.statements.last().expect("Just inserted").1,
						"Noted new statement"
					);
					// This will always return `Some` because it was just inserted.
					let key_value = self.statements
						.get_key_value(&comparator)
						.expect("Statement was just inserted; qed");

					NotedStatement::Fresh(key_value.into())
				}
			}
			CompactStatement::Valid(h) => {
				if !self.candidates.contains(&h) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Statement for unknown candidate"
					);
					return NotedStatement::NotUseful;
				}

				if let Some(old) = self.statements.insert(comparator.clone(), statement) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?old,
						"Known statement"
					);
					NotedStatement::UsefulButKnown
				} else {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?self.statements.last().expect("Just inserted").1,
						"Noted new statement"
					);
					// This will always return `Some` because it was just inserted.
					NotedStatement::Fresh(
						self.statements
							.get_key_value(&comparator)
							.expect("Statement was just inserted; qed")
							.into()
					)
				}
			}
		}
	}

	/// Returns an error if the statement is already known or not useful
	/// without modifying the internal state.
	fn check_useful_or_unknown(&self, statement: &UncheckedSignedFullStatement)
		-> std::result::Result<(), DeniedStatement>
	{
		let validator_index = statement.unchecked_validator_index();
		let compact = statement.unchecked_payload().to_compact();
		let comparator = StoredStatementComparator {
			compact: compact.clone(),
			validator_index,
			signature: statement.unchecked_signature().clone(),
		};

		match compact {
			CompactStatement::Seconded(_) => {
				let seconded_so_far = self.seconded_counts.get(&validator_index).unwrap_or(&0);
				if *seconded_so_far >= VC_THRESHOLD {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Extra statement is ignored",
					);
					return Err(DeniedStatement::NotUseful);
				}

				if self.statements.contains_key(&comparator) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Known statement",
					);
					return Err(DeniedStatement::UsefulButKnown);
				}
			}
			CompactStatement::Valid(h) => {
				if !self.candidates.contains(&h) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Statement for unknown candidate",
					);
					return Err(DeniedStatement::NotUseful);
				}

				if self.statements.contains_key(&comparator) {
					tracing::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Known statement",
					);
					return Err(DeniedStatement::UsefulButKnown);
				}
			}
		}
		Ok(())
	}

	/// Get an iterator over all statements for the active head. Seconded statements come first.
	fn statements(&self) -> impl Iterator<Item = StoredStatement<'_>> + '_ {
		self.statements.iter().map(Into::into)
	}

	/// Get an iterator over all statements for the active head that are for a particular candidate.
	fn statements_about(&self, candidate_hash: CandidateHash)
		-> impl Iterator<Item = StoredStatement<'_>> + '_ {
		self.statements().filter(move |s| s.compact().candidate_hash() == &candidate_hash)
	}
}

/// Check a statement signature under this parent hash.
fn check_statement_signature(
	head: &ActiveHeadData,
	relay_parent: Hash,
	statement: UncheckedSignedFullStatement,
) -> std::result::Result<SignedFullStatement, UncheckedSignedFullStatement> {
	let signing_context = SigningContext {
		session_index: head.session_index,
		parent_hash: relay_parent,
	};

	head.validators
		.get(statement.unchecked_validator_index().0 as usize)
		.ok_or_else(|| statement.clone())
		.and_then(|v| statement.try_into_checked(&signing_context, v))
}

/// Places the statement in storage if it is new, and then
/// circulates the statement to all peers who have not seen it yet, and
/// sends all statements dependent on that statement to peers who could previously not receive
/// them but now can.
#[tracing::instrument(level = "trace", skip(peers, ctx, active_heads, metrics), fields(subsystem = LOG_TARGET))]
async fn circulate_statement_and_dependents(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
	statement: SignedFullStatement,
	priority_peers: Vec<PeerId>,
	metrics: &Metrics,
) {
	let active_head = match active_heads.get_mut(&relay_parent) {
		Some(res) => res,
		None => return,
	};

	let _span = active_head.span.child("circulate-statement")
		.with_candidate(statement.payload().candidate_hash())
		.with_stage(jaeger::Stage::StatementDistribution);

	// First circulate the statement directly to all peers needing it.
	// The borrow of `active_head` needs to encompass only this (Rust) statement.
	let outputs: Option<(CandidateHash, Vec<PeerId>)> = {
		match active_head.note_statement(statement) {
			NotedStatement::Fresh(stored) =>
			{
				Some((
					*stored.compact().candidate_hash(),
					circulate_statement(peers, ctx, relay_parent, stored, priority_peers).await,
				))
			},
			_ => None,
		}
	};

	let _span = _span.child("send-to-peers");
	// Now send dependent statements to all peers needing them, if any.
	if let Some((candidate_hash, peers_needing_dependents)) = outputs {
		for peer in peers_needing_dependents {
			if let Some(peer_data) = peers.get_mut(&peer) {
				let _span_loop = _span.child("to-peer")
					.with_peer_id(&peer);
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
	let msg = if is_statement_large(&statement) {
		protocol_v1::StatementDistributionMessage::LargeStatement(
			StatementMetadata {
				relay_parent,
				candidate_hash: statement.payload().candidate_hash(),
				signed_by: statement.validator_index(),
				signature: statement.signature().clone(),
			}
		)
	} else {
		protocol_v1::StatementDistributionMessage::Statement(relay_parent, statement.into())
	};

	protocol_v1::ValidationProtocol::StatementDistribution(msg)
}

/// Check whether a statement should be treated as large statement.
fn is_statement_large(statement: &SignedFullStatement) -> bool {
	match &statement.payload() {
		Statement::Seconded(committed) => {
			// Runtime upgrades will always be large and even if not - no harm done.
			if committed.commitments.new_validation_code.is_some() {
				return true
			}
			// No runtime upgrade, now we need to be more nuanced:
			let size = statement.as_unchecked().encoded_size();

			// Half max size seems to be a good threshold to start not using notifications:
			let threshold =
				PeerSet::Validation.get_info(IsAuthority::Yes)
					.max_notification_size as usize / 2;

			size >= threshold
		}
		Statement::Valid(_) =>
			false,
	}
}

/// Circulates a statement to all peers who have not seen it yet, and returns
/// an iterator over peers who need to have dependent statements sent.
#[tracing::instrument(level = "trace", skip(peers, ctx), fields(subsystem = LOG_TARGET))]
async fn circulate_statement<'a>(
	peers: &mut HashMap<PeerId, PeerData>,
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
	stored: StoredStatement<'a>,
	mut priority_peers: Vec<PeerId>,
) -> Vec<PeerId> {
	let fingerprint = stored.fingerprint();

	let mut peers_to_send: Vec<PeerId> = peers.iter().filter_map(|(peer, data)| {
		if data.can_send(&relay_parent, &fingerprint) {
			Some(peer.clone())
		} else {
			None
		}
	}).collect();

	let good_peers: HashSet<&PeerId> = peers_to_send.iter().collect();
	// Only take priority peers we can send data to:
	priority_peers.retain(|p| good_peers.contains(p));

	// Avoid duplicates:
	let priority_set: HashSet<&PeerId> = priority_peers.iter().collect();
	peers_to_send.retain(|p| !priority_set.contains(p));

	let mut peers_to_send =
		util::choose_random_sqrt_subset(peers_to_send, MIN_GOSSIP_PEERS);
	// We don't want to use less peers, than we would without any priority peers:
	let min_size = std::cmp::max(peers_to_send.len(), MIN_GOSSIP_PEERS);
	// Make set full:
	let needed_peers = min_size as i64 - priority_peers.len() as i64;
	if needed_peers > 0 {
		peers_to_send.truncate(needed_peers as usize);
		// Order important here - priority peers are placed first, so will be sent first.
		// This gives backers a chance to be among the first in requesting any large statement
		// data.
		priority_peers.append(&mut peers_to_send);
	}
	peers_to_send = priority_peers;
	// We must not have duplicates:
	debug_assert!(
		peers_to_send.len() == peers_to_send.clone().into_iter().collect::<HashSet<_>>().len(),
		"We filter out duplicates above. qed.",
	);
	let peers_to_send: Vec<(PeerId, bool)> = peers_to_send.into_iter()
		.map(|peer_id| {
			let new = peers.get_mut(&peer_id)
				.expect("a subset is taken above, so it exists; qed")
				.send(&relay_parent, &fingerprint);
			(peer_id, new)
		}).collect();

	// Send all these peers the initial statement.
	if !peers_to_send.is_empty() {
		let payload = statement_message(relay_parent, stored.statement.clone());
		tracing::trace!(
			target: LOG_TARGET,
			?peers_to_send,
			?relay_parent,
			statement = ?stored.statement,
			"Sending statement",
		);
		ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
			peers_to_send.iter().map(|(p, _)| p.clone()).collect(),
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
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	active_head: &ActiveHeadData,
	metrics: &Metrics,
) {
	for statement in active_head.statements_about(candidate_hash) {
		let fingerprint = statement.fingerprint();
		if !peer_data.can_send(&relay_parent, &fingerprint) {
			continue;
		}
		peer_data.send(&relay_parent, &fingerprint);
		let payload = statement_message(
			relay_parent,
			statement.statement.clone(),
		);

		tracing::trace!(
			target: LOG_TARGET,
			?peer,
			?relay_parent,
			?candidate_hash,
			statement = ?statement.statement,
			"Sending statement",
		);
		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendValidationMessage(vec![peer.clone()], payload)
		)).await;

		metrics.on_statement_distributed();
	}
}

/// Send all statements at a given relay-parent to a peer.
#[tracing::instrument(level = "trace", skip(peer_data, ctx, active_head, metrics), fields(subsystem = LOG_TARGET))]
async fn send_statements(
	peer: PeerId,
	peer_data: &mut PeerData,
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
	active_head: &ActiveHeadData,
	metrics: &Metrics,
) {
	for statement in active_head.statements() {
		let fingerprint = statement.fingerprint();
		if !peer_data.can_send(&relay_parent, &fingerprint) {
			continue;
		}
		peer_data.send(&relay_parent, &fingerprint);
		let payload = statement_message(
			relay_parent,
			statement.statement.clone(),
		);

		tracing::trace!(
			target: LOG_TARGET,
			?peer,
			?relay_parent,
			statement = ?statement.statement,
			"Sending statement"
		);
		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendValidationMessage(vec![peer.clone()], payload)
		)).await;

		metrics.on_statement_distributed();
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

/// If message contains a statement, then retrieve it, otherwise fork task to fetch it.
///
/// This function will also return `None` if the message did not pass some basic checks, in that
/// case no statement will be requested, on the flipside you get `ActiveHeadData` in addition to
/// your statement.
///
/// If the message was large, but the result has been fetched already that one is returned.
async fn retrieve_statement_from_message<'a>(
	peer: PeerId,
	message: protocol_v1::StatementDistributionMessage,
	active_head: &'a mut ActiveHeadData,
	ctx: &mut impl SubsystemContext,
	req_sender: &mpsc::Sender<RequesterMessage>,
	metrics: &Metrics,
) -> Option<UncheckedSignedFullStatement> {

	let fingerprint = message.get_fingerprint();
	let candidate_hash = *fingerprint.0.candidate_hash();

	// Immediately return any Seconded statement:
	let message =
		if let protocol_v1::StatementDistributionMessage::Statement(h, s) = message {
			if let Statement::Seconded(_) = s.unchecked_payload() {
				return Some(s)
			}
			protocol_v1::StatementDistributionMessage::Statement(h, s)
		} else {
			message
		};

	match active_head.waiting_large_statements.entry(candidate_hash) {
		Entry::Occupied(mut occupied) => {
			match occupied.get_mut() {
				LargeStatementStatus::Fetching(info) => {

					let is_large_statement = message.is_large_statement();

					let is_new_peer =
						match info.available_peers.entry(peer) {
							IEntry::Occupied(mut occupied) => {
								occupied.get_mut().push(message);
								false
							}
							IEntry::Vacant(vacant) => {
								vacant.insert(vec![message]);
								true
							}
					};

					if is_new_peer & is_large_statement {
						info.peers_to_try.push(peer);
						// Answer any pending request for more peers:
						if let Some(sender) = info.peer_sender.take() {
							let to_send = std::mem::take(&mut info.peers_to_try);
							if let Err(peers) = sender.send(to_send) {
								// Requester no longer interested for now, might want them
								// later:
								info.peers_to_try = peers;
							}
						}
					}
				}
				LargeStatementStatus::FetchedOrShared(committed) => {
					match message {
						protocol_v1::StatementDistributionMessage::Statement(_, s) => {
							// We can now immediately return any statements (should only be
							// `Statement::Valid` ones, but we don't care at this point.)
							return Some(s)
						}
						protocol_v1::StatementDistributionMessage::LargeStatement(metadata) => {
							return Some(UncheckedSignedFullStatement::new(
								Statement::Seconded(
									committed.clone()),
									metadata.signed_by,
									metadata.signature.clone(),
							))
						}
					}
				}
			}
		}
		Entry::Vacant(vacant) => {
			match message {
				protocol_v1::StatementDistributionMessage::LargeStatement(metadata) => {
					if let Some(new_status) = launch_request(
						metadata,
						peer,
						req_sender.clone(),
						ctx,
						metrics
					).await {
						vacant.insert(new_status);
					}
				} 
				protocol_v1::StatementDistributionMessage::Statement(_, s) => {
					// No fetch in progress, safe to return any statement immediately (we don't bother
					// about normal network jitter which might cause `Valid` statements to arrive early
					// for now.).
					return Some(s)
				}
			}
		}
	}
	None
}

/// Launch request for a large statement and get tracking status.
///
/// Returns `None` if spawning task failed.
async fn launch_request(
	meta: StatementMetadata,
	peer: PeerId,
	req_sender: mpsc::Sender<RequesterMessage>,
	ctx: &mut impl SubsystemContext,
	metrics: &Metrics,
) -> Option<LargeStatementStatus> {

	let (task, handle) = fetch(
		meta.relay_parent,
		meta.candidate_hash,
		vec![peer],
		req_sender,
		metrics.clone(),
	)
	.remote_handle();

	let result = ctx.spawn("large-statement-fetcher", task.boxed())
		.await;
	if let Err(err) = result {
		tracing::error!(target: LOG_TARGET, ?err, "Spawning task failed.");
		return None
	}
	let available_peers = {
		let mut m = IndexMap::new();
		m.insert(peer, vec![protocol_v1::StatementDistributionMessage::LargeStatement(meta)]);
		m
	};
	Some(LargeStatementStatus::Fetching(FetchingInfo {
		available_peers,
		peers_to_try: Vec::new(),
		peer_sender: None,
		fetching_task: handle,
	}))
}

/// Handle incoming message and circulate it to peers, if we did not know it already.
///
async fn handle_incoming_message_and_circulate<'a>(
	peer: PeerId,
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &'a mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext,
	message: protocol_v1::StatementDistributionMessage,
	req_sender: &mpsc::Sender<RequesterMessage>,
	metrics: &Metrics,
) {
	let handled_incoming = match peers.get_mut(&peer) {
		Some(data) => {
			handle_incoming_message(
				peer,
				data,
				active_heads,
				ctx,
				message,
				req_sender,
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
			Vec::new(),
		).await;
	}
}

// Handle a statement. Returns a reference to a newly-stored statement
// if we were not already aware of it, along with the corresponding relay-parent.
//
// This function checks the signature and ensures the statement is compatible with our
// view. It also notifies candidate backing if the statement was previously unknown.
async fn handle_incoming_message<'a>(
	peer: PeerId,
	peer_data: &mut PeerData,
	active_heads: &'a mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext,
	message: protocol_v1::StatementDistributionMessage,
	req_sender: &mpsc::Sender<RequesterMessage>,
	metrics: &Metrics,
) -> Option<(Hash, StoredStatement<'a>)> {
	let relay_parent = message.get_relay_parent();

	let active_head = match active_heads.get_mut(&relay_parent) {
		Some(h) => h,
		None => {
			tracing::debug!(
				target: LOG_TARGET,
				%relay_parent,
				"our view out-of-sync with active heads; head not found",
			);
			report_peer(ctx, peer, COST_UNEXPECTED_STATEMENT).await;
			return None
		}
	};

	if let protocol_v1::StatementDistributionMessage::LargeStatement(_) = message {
		if let Err(rep) = peer_data.receive_large_statement(&relay_parent) {
			tracing::debug!(
				target: LOG_TARGET,
				?peer,
				?message,
				?rep,
				"Unexpected large statement.",
			);
			report_peer(ctx, peer, rep).await;
			return None;
		}
	}

	let fingerprint = message.get_fingerprint();
	let candidate_hash = fingerprint.0.candidate_hash().clone();
	let handle_incoming_span = active_head.span.child("handle-incoming")
		.with_candidate(candidate_hash)
		.with_peer_id(&peer);

	let max_message_count = active_head.validators.len() * 2;

	// perform only basic checks before verifying the signature
	// as it's more computationally heavy
	if let Err(rep) = peer_data.check_can_receive(&relay_parent, &fingerprint, max_message_count) {
		tracing::debug!(
			target: LOG_TARGET,
			?peer,
			?message,
			?rep,
			"Error inserting received statement"
		);
		report_peer(ctx, peer, rep).await;
		return None;
	}

	let statement = retrieve_statement_from_message(
		peer,
		message,
		active_head,
		ctx,
		req_sender,
		metrics,
	).await?;

	match active_head.check_useful_or_unknown(&statement) {
		Ok(()) => {},
		Err(DeniedStatement::NotUseful) => {
			return None;
		}
		Err(DeniedStatement::UsefulButKnown) => {
			report_peer(ctx, peer, BENEFIT_VALID_STATEMENT).await;
			return None;
		}
	}

	// check the signature on the statement.
	let statement = match check_statement_signature(&active_head, relay_parent, statement) {
		Err(statement) => {
			tracing::debug!(
				target: LOG_TARGET,
				?peer,
				?statement,
				"Invalid statement signature"
			);
			report_peer(ctx, peer, COST_INVALID_SIGNATURE).await;
			return None
		}
		Ok(statement) => statement,
	};

	// Ensure the statement is stored in the peer data.
	//
	// Note that if the peer is sending us something that is not within their view,
	// it will not be kept within their log.
	match peer_data.receive(&relay_parent, &fingerprint, max_message_count) {
		Err(_) => {
			unreachable!("checked in `check_can_receive` above; qed");
		}
		Ok(true) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer,
				?statement,
				"Statement accepted"
			);
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
		NotedStatement::NotUseful |
		NotedStatement::UsefulButKnown => {
			unreachable!("checked in `is_useful_or_unknown` above; qed");
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
	ctx: &mut impl SubsystemContext,
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

async fn handle_network_update(
	peers: &mut HashMap<PeerId, PeerData>,
	authorities: &mut HashMap<AuthorityDiscoveryId, PeerId>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext,
	req_sender: &mpsc::Sender<RequesterMessage>,
	update: NetworkBridgeEvent<protocol_v1::StatementDistributionMessage>,
	metrics: &Metrics,
) {
	match update {
		NetworkBridgeEvent::PeerConnected(peer, role, maybe_authority) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer,
				?role,
				"Peer connected",
			);
			peers.insert(peer, PeerData {
				view: Default::default(),
				view_knowledge: Default::default(),
				maybe_authority: maybe_authority.clone(),
			});
			if let Some(authority) = maybe_authority {
				authorities.insert(authority, peer);
			}
		}
		NetworkBridgeEvent::PeerDisconnected(peer) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer,
				"Peer disconnected",
			);
			if let Some(auth_id) = peers.remove(&peer).and_then(|p| p.maybe_authority) {
				authorities.remove(&auth_id);
			}
		}
		NetworkBridgeEvent::PeerMessage(peer, message) => {
			handle_incoming_message_and_circulate(
				peer,
				peers,
				active_heads,
				ctx,
				message,
				req_sender,
				metrics,
			).await;
		}
		NetworkBridgeEvent::PeerViewChange(peer, view) => {
			tracing::trace!(
				target: LOG_TARGET,
				?peer,
				?view,
				"Peer view change",
			);
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
		NetworkBridgeEvent::OurViewChange(_view) => {
			// handled by `ActiveLeavesUpdate`
		}
	}
}

impl StatementDistribution {
	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run(
		self,
		mut ctx: impl SubsystemContext<Message = StatementDistributionMessage>,
	) -> std::result::Result<(), Fatal> {
		let mut peers: HashMap<PeerId, PeerData> = HashMap::new();
		let mut authorities: HashMap<AuthorityDiscoveryId, PeerId> = HashMap::new();
		let mut active_heads: HashMap<Hash, ActiveHeadData> = HashMap::new();

		let mut runtime = RuntimeInfo::new(Some(self.keystore.clone()));

		// Sender/Receiver for getting news from our statement fetching tasks.
		let (req_sender, mut req_receiver) = mpsc::channel(1);
		// Sender/Receiver for getting news from our responder task.
		let (res_sender, mut res_receiver) = mpsc::channel(1);

		loop {
			let message = Message::receive(&mut ctx, &mut req_receiver, &mut res_receiver).await;
			match message {
				Message::Subsystem(result) => {
					let result = self.handle_subsystem_message(
						&mut ctx,
						&mut runtime,
						&mut peers,
						&mut authorities,
						&mut active_heads,
						&req_sender,
						&res_sender,
						result?,
					)
					.await;
					match result {
						Ok(true) => break,
						Ok(false) => {}
						Err(Error(Fault::Fatal(f))) => return Err(f), 
						Err(Error(Fault::Err(error))) =>
							tracing::debug!(target: LOG_TARGET, ?error)
					}
				}
				Message::Requester(result) => {
					let result = self.handle_requester_message(
						&mut ctx,
						&mut peers,
						&mut active_heads,
						&req_sender,
						result.ok_or(Fatal::RequesterReceiverFinished)?
					)
					.await;
					log_error(result.map_err(From::from), "handle_requester_message")?;
				}
				Message::Responder(result) => {
					let result = self.handle_responder_message(
						&peers,
						&mut active_heads,
						result.ok_or(Fatal::ResponderReceiverFinished)?
					)
					.await;
					log_error(result.map_err(From::from), "handle_responder_message")?;
				}
			};
		}
		Ok(())
	}

	/// Handle messages from responder background task.
	async fn handle_responder_message(
		&self,
		peers: &HashMap<PeerId, PeerData>,
		active_heads: &mut HashMap<Hash, ActiveHeadData>,
		message: ResponderMessage,
	) -> NonFatalResult<()> {
		match message {
			ResponderMessage::GetData {
				requesting_peer,
				relay_parent,
				candidate_hash,
				tx,
			} => {
				if !requesting_peer_knows_about_candidate(
					peers,
					&requesting_peer,
					&relay_parent,
					&candidate_hash
				) {
					return Err(
						NonFatal::RequestedUnannouncedCandidate(requesting_peer, candidate_hash)
					)
				}

				let active_head = active_heads
						.get(&relay_parent)
						.ok_or(NonFatal::NoSuchHead(relay_parent))?;

				let committed = match active_head.waiting_large_statements.get(&candidate_hash) {
					Some(LargeStatementStatus::FetchedOrShared(committed)) => committed.clone(),
					_ => {
						return Err(
							NonFatal::NoSuchFetchedLargeStatement(relay_parent, candidate_hash)
						)
					}
				};

				tx.send(committed).map_err(|_| NonFatal::ResponderGetDataCanceled)?;
			}
		}
		Ok(())
	}

	async fn handle_requester_message(
		&self,
		ctx: &mut impl SubsystemContext,
		peers: &mut HashMap<PeerId, PeerData>,
		active_heads: &mut HashMap<Hash, ActiveHeadData>,
		req_sender: &mpsc::Sender<RequesterMessage>,
		message: RequesterMessage,
	) -> NonFatalResult<()> {
		match message {
			RequesterMessage::Finished {
				relay_parent,
				candidate_hash,
				from_peer,
				response,
				bad_peers,
			} => {
				for bad in bad_peers {
					report_peer(ctx, bad, COST_FETCH_FAIL).await;
				}
				report_peer(ctx, from_peer, BENEFIT_VALID_RESPONSE).await;

				let active_head = active_heads
					.get_mut(&relay_parent)
					.ok_or(NonFatal::NoSuchHead(relay_parent))?;

				let status = active_head
					.waiting_large_statements
					.remove(&candidate_hash);

				let info = match status {
					Some(LargeStatementStatus::Fetching(info)) => info,
					Some(LargeStatementStatus::FetchedOrShared(_)) => {
						// We are no longer interested in the data.
						return Ok(())
					}
					None => {
						return Err(
							NonFatal::NoSuchLargeStatementStatus(relay_parent, candidate_hash)
						)
					}
				};

				active_head.waiting_large_statements.insert(
					candidate_hash,
					LargeStatementStatus::FetchedOrShared(response),
				);

				// Cache is now populated, send all messages:
				for (peer, messages) in info.available_peers {
					for message in messages {
						handle_incoming_message_and_circulate(
							peer,
							peers,
							active_heads,
							ctx,
							message,
							req_sender,
							&self.metrics,
						)
						.await;
					}
				}
			}
			RequesterMessage::SendRequest(req) => {
				ctx.send_message(
					AllMessages::NetworkBridge(
						NetworkBridgeMessage::SendRequests(
							vec![req],
							IfDisconnected::ImmediateError,
						)
					))
					.await;
			}
			RequesterMessage::GetMorePeers {
				relay_parent,
				candidate_hash,
				tx,
			} => {
				let active_head = active_heads
					.get_mut(&relay_parent)
					.ok_or(NonFatal::NoSuchHead(relay_parent))?;

				let status = active_head
					.waiting_large_statements
					.get_mut(&candidate_hash);

				let info = match status {
					Some(LargeStatementStatus::Fetching(info)) => info,
					Some(LargeStatementStatus::FetchedOrShared(_)) => {
						// This task is going to die soon - no need to send it anything.
						tracing::debug!(
							target: LOG_TARGET,
							"Zombie task wanted more peers."
						);
						return Ok(())
					}
					None => {
						return Err(
							NonFatal::NoSuchLargeStatementStatus(relay_parent, candidate_hash)
						)
					}
				};

				if info.peers_to_try.is_empty() {
					info.peer_sender = Some(tx);
				} else {
					let peers_to_try = std::mem::take(&mut info.peers_to_try);
					if let Err(peers) = tx.send(peers_to_try) {
						// No longer interested for now - might want them later:
						info.peers_to_try = peers;
					}
				}
			}
			RequesterMessage::ReportPeer(peer, rep) =>
				report_peer(ctx, peer, rep).await,
		}
		Ok(())
	}


	async fn handle_subsystem_message(
		&self,
		ctx: &mut impl SubsystemContext,
		runtime: &mut RuntimeInfo,
		peers: &mut HashMap<PeerId, PeerData>,
		authorities: &mut HashMap<AuthorityDiscoveryId, PeerId>,
		active_heads: &mut HashMap<Hash, ActiveHeadData>,
		req_sender: &mpsc::Sender<RequesterMessage>,
		res_sender: &mpsc::Sender<ResponderMessage>,
		message: FromOverseer<StatementDistributionMessage>,
	) -> Result<bool> {
		let metrics = &self.metrics;

		match message {
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated })) => {
				let _timer = metrics.time_active_leaves_update();

				for activated in activated {
					let relay_parent = activated.hash;
					let span = PerLeafSpan::new(activated.span, "statement-distribution");
					tracing::trace!(
						target: LOG_TARGET,
						hash = ?relay_parent,
						"New active leaf",
					);

					let session_index = runtime.get_session_index(ctx, relay_parent).await?;
					let info = runtime.get_session_info_by_index(ctx, relay_parent, session_index).await?;
					let session_info = &info.session_info;

					active_heads.entry(relay_parent)
						.or_insert(ActiveHeadData::new(session_info.validators.clone(), session_index, span));

					active_heads.retain(|h, _| {
						let live = !deactivated.contains(h);
						if !live {
							tracing::trace!(
								target: LOG_TARGET,
								hash = ?h,
								"Deactivating leaf",
							);
						}
						live
					});
				}
			}
			FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {
				// do nothing
			}
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(true),
			FromOverseer::Communication { msg } => match msg {
				StatementDistributionMessage::Share(relay_parent, statement) => {
					let _timer = metrics.time_share();

					// Make sure we have data in cache:
					if is_statement_large(&statement) {
						if let Statement::Seconded(committed) = &statement.payload() {
							let active_head = active_heads
								.get_mut(&relay_parent)
								// This should never be out-of-sync with our view if the view
								// updates correspond to actual `StartWork` messages.
								.ok_or(NonFatal::NoSuchHead(relay_parent))?;
							active_head.waiting_large_statements.insert(
								statement.payload().candidate_hash(),
								LargeStatementStatus::FetchedOrShared(committed.clone())
							);
						}
					}

					let info = runtime.get_session_info(ctx, relay_parent).await?;
					let session_info = &info.session_info;
					let validator_info = &info.validator_info;

					// Get peers in our group, so we can make sure they get our statement
					// directly:
					let group_peers = {
						if let Some(our_group) = validator_info.our_group {
							let our_group = &session_info.validator_groups[our_group.0 as usize];

							our_group.into_iter()
								.filter_map(|i| {
									if Some(*i) == validator_info.our_index {
										return None
									}
									let authority_id = &session_info.discovery_keys[i.0 as usize];
									authorities.get(authority_id).map(|p| *p)
								})
								.collect()
						} else {
							Vec::new()
						}
					};
					circulate_statement_and_dependents(
						peers,
						active_heads,
						ctx,
						relay_parent,
						statement,
						group_peers,
						metrics,
					).await;
				}
				StatementDistributionMessage::NetworkBridgeUpdateV1(event) => {
					let _timer = metrics.time_network_bridge_update_v1();

					handle_network_update(
						peers,
						authorities,
						active_heads,
						ctx,
						req_sender,
						event,
						metrics,
					).await;
				}
				StatementDistributionMessage::StatementFetchingReceiver(receiver) => {
					ctx.spawn(
						"large-statement-responder",
						respond(receiver, res_sender.clone()).boxed()
					)
					.await
					.map_err(Fatal::SpawnTask)?;
				}
			}
		}
		Ok(false)
	}
}

/// Check whether a peer knows about a candidate from us.
///
/// If not, it is deemed illegal for it to request corresponding data from us.
fn requesting_peer_knows_about_candidate(
	peers: &HashMap<PeerId, PeerData>,
	requesting_peer: &PeerId,
	relay_parent: &Hash,
	candidate_hash: &CandidateHash,
) -> bool {
	requesting_peer_knows_about_candidate_inner(
		peers,
		requesting_peer,
		relay_parent,
		candidate_hash,
	).is_some()
}

/// Helper function for `requesting_peer_knows_about_statement`.
fn requesting_peer_knows_about_candidate_inner(
	peers: &HashMap<PeerId, PeerData>,
	requesting_peer: &PeerId,
	relay_parent: &Hash,
	candidate_hash: &CandidateHash,
) -> Option<()> {
	let peer_data = peers.get(requesting_peer)?;
	let knowledge = peer_data.view_knowledge.get(relay_parent)?;
	knowledge.sent_candidates.get(&candidate_hash)?;
	Some(())
}

#[derive(Clone)]
struct MetricsInner {
	statements_distributed: prometheus::Counter<prometheus::U64>,
	sent_requests: prometheus::Counter<prometheus::U64>,
	received_responses: prometheus::CounterVec<prometheus::U64>,
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

	fn on_sent_request(&self) {
		if let Some(metrics) = &self.0 {
			metrics.sent_requests.inc();
		}
	}

	fn on_received_response(&self, success: bool) {
		if let Some(metrics) = &self.0 {
			let label = if success { "succeeded" } else { "failed" };
			metrics.received_responses.with_label_values(&[label]).inc();
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
			sent_requests: prometheus::register(
				prometheus::Counter::new(
					"parachain_statement_distribution_sent_requests_total",
					"Number of large statement fetching requests sent."
				)?,
				registry,
			)?,
			received_responses: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_statement_distribution_received_responses_total",
						"Number of received responses for large statement data."
					),
					&["success"],
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
	use std::time::Duration;
	use parity_scale_codec::{Decode, Encode};
	use super::*;
	use std::sync::Arc;
	use sp_keyring::Sr25519Keyring;
	use sp_application_crypto::{AppKey, sr25519::Pair, Pair as TraitPair};
	use polkadot_node_primitives::Statement;
	use polkadot_primitives::v1::{CommittedCandidateReceipt, ValidationCode, SessionInfo};
	use assert_matches::assert_matches;
	use futures::executor::{self, block_on};
	use futures_timer::Delay;
	use sp_keystore::{CryptoStore, SyncCryptoStorePtr, SyncCryptoStore};
	use sc_keystore::LocalKeystore;
	use polkadot_node_network_protocol::{view, ObservedRole, request_response::Recipient};
	use polkadot_subsystem::{jaeger, ActivatedLeaf, messages::{RuntimeApiMessage, RuntimeApiRequest}};
	use polkadot_node_network_protocol::request_response::{
		Requests,
		v1::{
			StatementFetchingRequest,
			StatementFetchingResponse,
		},
	};

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
		assert!(head_data.check_useful_or_unknown(&a_seconded_val_0.clone().into()).is_ok());
		let noted = head_data.note_statement(a_seconded_val_0.clone());

		assert_matches!(noted, NotedStatement::Fresh(_));

		// note A (duplicate)
		assert_eq!(
			head_data.check_useful_or_unknown(&a_seconded_val_0.clone().into()),
			Err(DeniedStatement::UsefulButKnown),
		);
		let noted = head_data.note_statement(a_seconded_val_0);

		assert_matches!(noted, NotedStatement::UsefulButKnown);

		// note B
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_b.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)).ok().flatten().expect("should be signed");
		assert!(head_data.check_useful_or_unknown(&statement.clone().into()).is_ok());
		let noted = head_data.note_statement(statement);
		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (beyond 2 - ignored)
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)).ok().flatten().expect("should be signed");
		assert_eq!(
			head_data.check_useful_or_unknown(&statement.clone().into()),
			Err(DeniedStatement::NotUseful),
		);
		let noted = head_data.note_statement(statement);
		assert_matches!(noted, NotedStatement::NotUseful);

		// note B (new validator)
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_b.clone()),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		)).ok().flatten().expect("should be signed");
		assert!(head_data.check_useful_or_unknown(&statement.clone().into()).is_ok());
		let noted = head_data.note_statement(statement);
		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (new validator)
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		)).ok().flatten().expect("should be signed");
		assert!(head_data.check_useful_or_unknown(&statement.clone().into()).is_ok());
		let noted = head_data.note_statement(statement);
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
		assert!(!knowledge.can_send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))));
		assert!(!knowledge.is_known_candidate(&hash_a));
		assert!(knowledge.sent_statements.is_empty());
		assert!(knowledge.received_statements.is_empty());
		assert!(knowledge.seconded_counts.is_empty());
		assert!(knowledge.received_message_count.is_empty());

		// Make the peer aware of the candidate.
		assert_eq!(knowledge.send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0))), true);
		assert_eq!(knowledge.send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(1))), false);
		assert!(knowledge.is_known_candidate(&hash_a));
		assert_eq!(knowledge.sent_statements.len(), 2);
		assert!(knowledge.received_statements.is_empty());
		assert_eq!(knowledge.seconded_counts.len(), 2);
		assert!(knowledge.received_message_count.get(&hash_a).is_none());

		// And now it should accept the dependent message.
		assert_eq!(knowledge.send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))), false);
		assert!(knowledge.is_known_candidate(&hash_a));
		assert_eq!(knowledge.sent_statements.len(), 3);
		assert!(knowledge.received_statements.is_empty());
		assert_eq!(knowledge.seconded_counts.len(), 2);
		assert!(knowledge.received_message_count.get(&hash_a).is_none());
	}

	#[test]
	fn cant_send_after_receiving() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());
		assert!(knowledge.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3).is_ok());
		assert!(knowledge.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3).unwrap());
		assert!(!knowledge.can_send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0))));
	}

	#[test]
	fn per_peer_relay_parent_knowledge_receive() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);

		assert!(knowledge.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3).is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3),
			Ok(true),
		);

		// Push statements up to the flood limit.
		assert!(knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3).is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3),
			Ok(false),
		);

		assert!(knowledge.is_known_candidate(&hash_a));
		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 2);

		assert!(knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3).is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Ok(false),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
			Err(COST_APPARENT_FLOOD),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
			Err(COST_APPARENT_FLOOD),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);
		assert_eq!(knowledge.received_statements.len(), 3); // number of prior `Ok`s.

		// Now make sure that the seconding limit is respected.
		let hash_b = CandidateHash([2; 32].into());
		let hash_c = CandidateHash([3; 32].into());

		assert!(knowledge.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3).is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
			Ok(true),
		);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
			Err(COST_UNEXPECTED_STATEMENT),
		);

		// Last, make sure that already-known statements are disregarded.
		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Err(COST_DUPLICATE_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Err(COST_DUPLICATE_STATEMENT),
		);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
			Err(COST_DUPLICATE_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
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

			let statement = block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)).ok().flatten().expect("should be signed");
			assert!(data.check_useful_or_unknown(&statement.clone().into()).is_ok());
			let noted = data.note_statement(statement);

			assert_matches!(noted, NotedStatement::Fresh(_));

			let statement = block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Valid(candidate_hash),
				&signing_context,
				ValidatorIndex(1),
				&bob_public.into(),
			)).ok().flatten().expect("should be signed");
			assert!(data.check_useful_or_unknown(&statement.clone().into()).is_ok());
			let noted = data.note_statement(statement);

			assert_matches!(noted, NotedStatement::Fresh(_));

			let statement = block_on(SignedFullStatement::sign(
				&keystore,
				Statement::Valid(candidate_hash),
				&signing_context,
				ValidatorIndex(2),
				&charlie_public.into(),
			)).ok().flatten().expect("should be signed");
			assert!(data.check_useful_or_unknown(&statement.clone().into()).is_ok());
			let noted = data.note_statement(statement);
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
			maybe_authority: None,
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			polkadot_node_subsystem_test_helpers
				::make_subsystem_context
				::<StatementDistributionMessage,_>(pool);
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

			assert!(c_knowledge.is_known_candidate(&candidate_hash));
			assert!(c_knowledge.sent_statements.contains(
				&(CompactStatement::Seconded(candidate_hash), ValidatorIndex(0))
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
			maybe_authority: None,
		};

		let mut peer_data: HashMap<_, _> = vec![
			(peer_a.clone(), peer_data_from_view(peer_a_view)),
			(peer_b.clone(), peer_data_from_view(peer_b_view)),
			(peer_c.clone(), peer_data_from_view(peer_c_view)),
		].into_iter().collect();

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			polkadot_node_subsystem_test_helpers
				::make_subsystem_context
				::<StatementDistributionMessage,_>(pool);

		executor::block_on(async move {
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

			let comparator = StoredStatementComparator {
					compact: statement.payload().to_compact(),
					validator_index: ValidatorIndex(0),
					signature: statement.signature().clone()
			};
			let statement = StoredStatement {
				comparator: &comparator,
				statement: &statement,
			};

			let needs_dependents = circulate_statement(
				&mut peer_data,
				&mut ctx,
				hash_b,
				statement,
				Vec::new(),
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
			Sr25519Keyring::Alice.pair(),
			Sr25519Keyring::Bob.pair(),
			Sr25519Keyring::Charlie.pair(),
		];

		let session_info = make_session_info(validators, vec![]);

		let session_index = 1;

		let pool = sp_core::testing::TaskExecutor::new();
		let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bg = async move {
			let s = StatementDistribution { metrics: Default::default(), keystore: Arc::new(LocalKeystore::in_memory()) };
			s.run(ctx).await.unwrap();
		};

		let test_fut = async move {
			// register our active heads.
			handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![ActivatedLeaf {
					hash: hash_a,
					number: 1,
					span: Arc::new(jaeger::Span::Disabled),
				}].into(),
				deactivated: vec![].into(),
			}))).await;

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

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
				)
					if r == hash_a && sess_index == session_index
				=> {
					let _ = tx.send(Ok(Some(session_info)));
				}
			);

			// notify of peers and view
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_a.clone(), ObservedRole::Full, None)
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full, None)
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
						protocol_v1::StatementDistributionMessage::Statement(hash_a, statement.clone().into()),
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
					assert_eq!(s, statement.into());
				}
			);
			handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::pin_mut!(test_fut);
		futures::pin_mut!(bg);

		executor::block_on(future::join(test_fut, bg));
	}

	#[test]
	fn receiving_large_statement_from_one_sends_to_another_and_to_candidate_backing() {
		sp_tracing::try_init_simple();
		let hash_a = Hash::repeat_byte(1);
		let hash_b = Hash::repeat_byte(2);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_a;
			c.descriptor.para_id = 1.into();
			c.commitments.new_validation_code = Some(ValidationCode(vec![1,2,3]));
			c
		};

		let peer_a = PeerId::random(); // Alice
		let peer_b = PeerId::random(); // Bob
		let peer_c = PeerId::random(); // Charlie
		let peer_bad = PeerId::random(); // No validator

		let validators = vec![
			Sr25519Keyring::Alice.pair(),
			Sr25519Keyring::Bob.pair(),
			Sr25519Keyring::Charlie.pair(),
			// We:
			Sr25519Keyring::Ferdie.pair(),
		];

		let session_info = make_session_info(
			validators,
			vec![vec![0,1,2,4], vec![3]]
		);

		let session_index = 1;

		let pool = sp_core::testing::TaskExecutor::new();
		let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bg = async move {
			let s = StatementDistribution { metrics: Default::default(), keystore: make_ferdie_keystore()};
			s.run(ctx).await.unwrap();
		};

		let (mut tx_reqs, rx_reqs) = mpsc::channel(1);

		let test_fut = async move {
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::StatementFetchingReceiver(rx_reqs)
			}).await;

			// register our active heads.
			handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![ActivatedLeaf {
					hash: hash_a,
					number: 1,
					span: Arc::new(jaeger::Span::Disabled),
				}].into(),
				deactivated: vec![].into(),
			}))).await;

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

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
				)
					if r == hash_a && sess_index == session_index
				=> {
					let _ = tx.send(Ok(Some(session_info)));
				}
			);

			// notify of peers and view
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Alice.public().into())
					)
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Bob.public().into())
					)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_c.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Charlie.public().into())
					)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_bad.clone(), ObservedRole::Full, None)
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
					NetworkBridgeEvent::PeerViewChange(peer_c.clone(), view![hash_a])
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_bad.clone(), view![hash_a])
				)
			}).await;

			// receive a seconded statement from peer A, which does not provide the request data,
			// then get that data from peer C. It should be propagated onwards to peer B and to
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
					Statement::Seconded(candidate.clone()),
					&signing_context,
					ValidatorIndex(0),
					&alice_public.into(),
				).await.ok().flatten().expect("should be signed")
			};

			let metadata =
				protocol_v1::StatementDistributionMessage::Statement(hash_a, statement.clone().into()).get_metadata();

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						protocol_v1::StatementDistributionMessage::LargeStatement(metadata.clone()),
					)
				)
			}).await;

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
					// Just drop request - should trigger error.
				}
			);

			// There is a race between request handler asking for more peers and processing of the
			// coming `PeerMessage`s, we want the request handler to ask first here for better test
			// coverage:
			Delay::new(Duration::from_millis(20)).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::StatementDistributionMessage::LargeStatement(metadata.clone()),
					)
				)
			}).await;

			// Malicious peer:
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_bad.clone(),
						protocol_v1::StatementDistributionMessage::LargeStatement(metadata.clone()),
					)
				)
			}).await;

			// Let c fail once too:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					assert_eq!(outgoing.peer, Recipient::Peer(peer_c));
				}
			);

			// a fails again:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					// On retry, we should have reverse order:
					assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
				}
			);

			// Send invalid response (all other peers have been tried now):
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					assert_eq!(outgoing.peer, Recipient::Peer(peer_bad));
					let bad_candidate = {
						let mut bad = candidate.clone();
						bad.descriptor.para_id = 0xeadbeaf.into();
						bad
					};
					let response = StatementFetchingResponse::Statement(bad_candidate);
					outgoing.pending_response.send(Ok(response.encode())).unwrap();
				}
			);

			// Should get punished and never tried again:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(p, r)
				) if p == peer_bad && r == COST_WRONG_HASH => {}
			);

			// a is tried again (retried in reverse order):
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					// On retry, we should have reverse order:
					assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
				}
			);

			// c succeeds now:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut reqs, IfDisconnected::ImmediateError
					)
				) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetching(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					// On retry, we should have reverse order:
					assert_eq!(outgoing.peer, Recipient::Peer(peer_c));
					let response = StatementFetchingResponse::Statement(candidate.clone());
					outgoing.pending_response.send(Ok(response.encode())).unwrap();
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(p, r)
				) if p == peer_a && r == COST_FETCH_FAIL => {}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(p, r)
				) if p == peer_c && r == BENEFIT_VALID_RESPONSE => {}
			);

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


			// Now messages should go out:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						mut recipients,
						protocol_v1::ValidationProtocol::StatementDistribution(
							protocol_v1::StatementDistributionMessage::LargeStatement(meta)
						),
					)
				) => {
					tracing::debug!(
						target: LOG_TARGET,
						?recipients,
						"Recipients received"
					);
					recipients.sort();
					let mut expected = vec![peer_b, peer_c, peer_bad];
					expected.sort();
					assert_eq!(recipients, expected);
					assert_eq!(meta.relay_parent, hash_a);
					assert_eq!(meta.candidate_hash, statement.payload().candidate_hash());
					assert_eq!(meta.signed_by, statement.validator_index());
					assert_eq!(&meta.signature, statement.signature());
				}
			);

			// Now that it has the candidate it should answer requests accordingly (even after a
			// failed request):

			// Failing request first (wrong relay parent hash):
			let (pending_response, response_rx) = oneshot::channel();
			let inner_req = StatementFetchingRequest {
				relay_parent: hash_b,
				candidate_hash: metadata.candidate_hash,
			};
			let req = sc_network::config::IncomingRequest {
				peer: peer_b,
				payload: inner_req.encode(),
				pending_response,
			};
			tx_reqs.send(req).await.unwrap();
			assert_matches!(
				response_rx.await.unwrap().result,
				Err(()) => {}
			);

			// Another failing request (peer_a never received a statement from us, so it is not
			// allowed to request the data):
			let (pending_response, response_rx) = oneshot::channel();
			let inner_req = StatementFetchingRequest {
				relay_parent: metadata.relay_parent,
				candidate_hash: metadata.candidate_hash,
			};
			let req = sc_network::config::IncomingRequest {
				peer: peer_a,
				payload: inner_req.encode(),
				pending_response,
			};
			tx_reqs.send(req).await.unwrap();
			assert_matches!(
				response_rx.await.unwrap().result,
				Err(()) => {}
			);

			// And now the succeding request from peer_b:
			let (pending_response, response_rx) = oneshot::channel();
			let inner_req = StatementFetchingRequest {
				relay_parent: metadata.relay_parent,
				candidate_hash: metadata.candidate_hash,
			};
			let req = sc_network::config::IncomingRequest {
				peer: peer_b,
				payload: inner_req.encode(),
				pending_response,
			};
			tx_reqs.send(req).await.unwrap();
			let StatementFetchingResponse::Statement(committed) =
				Decode::decode(&mut response_rx.await.unwrap().result.unwrap().as_ref()).unwrap();
			assert_eq!(committed, candidate);

			handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::pin_mut!(test_fut);
		futures::pin_mut!(bg);

		executor::block_on(future::join(test_fut, bg));
	}

	#[test]
	fn share_prioritizes_backing_group() {
		sp_tracing::try_init_simple();
		let hash_a = Hash::repeat_byte(1);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_a;
			c.descriptor.para_id = 1.into();
			c.commitments.new_validation_code = Some(ValidationCode(vec![1,2,3]));
			c
		};

		let peer_a = PeerId::random(); // Alice
		let peer_b = PeerId::random(); // Bob
		let peer_c = PeerId::random(); // Charlie
		let peer_bad = PeerId::random(); // No validator
		let peer_other_group = PeerId::random(); //Ferdie

		let mut validators = vec![
			Sr25519Keyring::Alice.pair(),
			Sr25519Keyring::Bob.pair(),
			Sr25519Keyring::Charlie.pair(),
			// other group
			Sr25519Keyring::Dave.pair(),
			// We:
			Sr25519Keyring::Ferdie.pair(),
		];

		// Strictly speaking we only need MIN_GOSSIP_PEERS - 3 to make sure only priority peers
		// will be served, but by using a larger value we test for overflow errors:
		let dummy_count = MIN_GOSSIP_PEERS;

		// We artificially inflate our group, so there won't be any free slots for other peers. (We
		// want to test that our group is prioritized):
		let dummy_pairs: Vec<_> = std::iter::repeat_with(|| Pair::generate().0).take(dummy_count).collect();
		let dummy_peers: Vec<_> = std::iter::repeat_with(|| PeerId::random()).take(dummy_count).collect();

		validators = validators.into_iter().chain(dummy_pairs.clone()).collect();

		let mut first_group = vec![0,1,2,4];
		first_group.append(&mut (0..dummy_count as u32).map(|v| v + 5).collect());
		let session_info = make_session_info(
			validators,
			vec![first_group, vec![3]]
		);

		let session_index = 1;

		let pool = sp_core::testing::TaskExecutor::new();
		let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bg = async move {
			let s = StatementDistribution { metrics: Default::default(), keystore: make_ferdie_keystore()};
			s.run(ctx).await.unwrap();
		};

		let (mut tx_reqs, rx_reqs) = mpsc::channel(1);

		let test_fut = async move {
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::StatementFetchingReceiver(rx_reqs)
			}).await;

			// register our active heads.
			handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![ActivatedLeaf {
					hash: hash_a,
					number: 1,
					span: Arc::new(jaeger::Span::Disabled),
				}].into(),
				deactivated: vec![].into(),
			}))).await;

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

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
				)
					if r == hash_a && sess_index == session_index
				=> {
					let _ = tx.send(Ok(Some(session_info)));
				}
			);

			// notify of dummy peers and view
			for (peer, pair) in dummy_peers.clone().into_iter().zip(dummy_pairs) {
				handle.send(FromOverseer::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerConnected(
							peer,
							ObservedRole::Full,
							Some(pair.public().into()),
						)
					)
				}).await;

				handle.send(FromOverseer::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerViewChange(peer, view![hash_a])
					)
				}).await;
			}

			// notify of peers and view
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Alice.public().into())
					)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Bob.public().into())
					)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_c.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Charlie.public().into())
					)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(peer_bad.clone(), ObservedRole::Full, None)
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_other_group.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Dave.public().into())
					)
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
					NetworkBridgeEvent::PeerViewChange(peer_c.clone(), view![hash_a])
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_bad.clone(), view![hash_a])
				)
			}).await;
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_other_group.clone(), view![hash_a])
				)
			}).await;

			// receive a seconded statement from peer A, which does not provide the request data,
			// then get that data from peer C. It should be propagated onwards to peer B and to
			// candidate backing.
			let statement = {
				let signing_context = SigningContext {
					parent_hash: hash_a,
					session_index,
				};

				let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
				let ferdie_public = CryptoStore::sr25519_generate_new(
					&*keystore, ValidatorId::ID, Some(&Sr25519Keyring::Ferdie.to_seed())
				).await.unwrap();

				SignedFullStatement::sign(
					&keystore,
					Statement::Seconded(candidate.clone()),
					&signing_context,
					ValidatorIndex(4),
					&ferdie_public.into(),
				).await.ok().flatten().expect("should be signed")
			};

			let metadata =
				protocol_v1::StatementDistributionMessage::Statement(hash_a, statement.clone().into()).get_metadata();

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::Share(hash_a, statement.clone())
			}).await;

			// Messages should go out:
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						mut recipients,
						protocol_v1::ValidationProtocol::StatementDistribution(
							protocol_v1::StatementDistributionMessage::LargeStatement(meta)
						),
					)
				) => {
					tracing::debug!(
						target: LOG_TARGET,
						?recipients,
						"Recipients received"
					);
					recipients.sort();
					// We expect only our backing group to be the recipients, du to the inflated
					// test group above:
					let mut expected: Vec<_> = vec![peer_a, peer_b, peer_c].into_iter().chain(dummy_peers).collect();
					expected.sort();
					assert_eq!(recipients.len(), expected.len());
					assert_eq!(recipients, expected);
					assert_eq!(meta.relay_parent, hash_a);
					assert_eq!(meta.candidate_hash, statement.payload().candidate_hash());
					assert_eq!(meta.signed_by, statement.validator_index());
					assert_eq!(&meta.signature, statement.signature());
				}
			);

			// Now that it has the candidate it should answer requests accordingly:

			let (pending_response, response_rx) = oneshot::channel();
			let inner_req = StatementFetchingRequest {
				relay_parent: metadata.relay_parent,
				candidate_hash: metadata.candidate_hash,
			};
			let req = sc_network::config::IncomingRequest {
				peer: peer_b,
				payload: inner_req.encode(),
				pending_response,
			};
			tx_reqs.send(req).await.unwrap();
			let StatementFetchingResponse::Statement(committed) =
				Decode::decode(&mut response_rx.await.unwrap().result.unwrap().as_ref()).unwrap();
			assert_eq!(committed, candidate);

			handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::pin_mut!(test_fut);
		futures::pin_mut!(bg);

		executor::block_on(future::join(test_fut, bg));
	}

	#[test]
	fn peer_cant_flood_with_large_statements() {
		sp_tracing::try_init_simple();
		let hash_a = Hash::repeat_byte(1);

		let candidate = {
			let mut c = CommittedCandidateReceipt::default();
			c.descriptor.relay_parent = hash_a;
			c.descriptor.para_id = 1.into();
			c.commitments.new_validation_code = Some(ValidationCode(vec![1,2,3]));
			c
		};

		let peer_a = PeerId::random(); // Alice

		let validators = vec![
			Sr25519Keyring::Alice.pair(),
			Sr25519Keyring::Bob.pair(),
			Sr25519Keyring::Charlie.pair(),
			// other group
			Sr25519Keyring::Dave.pair(),
			// We:
			Sr25519Keyring::Ferdie.pair(),
		];

		let first_group = vec![0,1,2,4];
		let session_info = make_session_info(
			validators,
			vec![first_group, vec![3]]
		);

		let session_index = 1;

		let pool = sp_core::testing::TaskExecutor::new();
		let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

		let bg = async move {
			let s = StatementDistribution { metrics: Default::default(), keystore: make_ferdie_keystore()};
			s.run(ctx).await.unwrap();
		};

		let (_, rx_reqs) = mpsc::channel(1);

		let test_fut = async move {
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::StatementFetchingReceiver(rx_reqs)
			}).await;

			// register our active heads.
			handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![ActivatedLeaf {
					hash: hash_a,
					number: 1,
					span: Arc::new(jaeger::Span::Disabled),
				}].into(),
				deactivated: vec![].into(),
			}))).await;

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

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
				)
					if r == hash_a && sess_index == session_index
				=> {
					let _ = tx.send(Ok(Some(session_info)));
				}
			);

			// notify of peers and view
			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						Some(Sr25519Keyring::Alice.public().into())
					)
				)
			}).await;

			handle.send(FromOverseer::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a])
				)
			}).await;
			
			// receive a seconded statement from peer A.
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
					Statement::Seconded(candidate.clone()),
					&signing_context,
					ValidatorIndex(0),
					&alice_public.into(),
				).await.ok().flatten().expect("should be signed")
			};

			let metadata =
				protocol_v1::StatementDistributionMessage::Statement(hash_a, statement.clone().into()).get_metadata();

			for _ in 0..MAX_LARGE_STATEMENTS_PER_SENDER + 1 {
				handle.send(FromOverseer::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdateV1(
						NetworkBridgeEvent::PeerMessage(
							peer_a.clone(),
							protocol_v1::StatementDistributionMessage::LargeStatement(metadata.clone()),
						)
					)
				}).await;
			}

			// We should try to fetch the data and punish the peer (but we don't know what comes
			// first):
			let mut requested = false;
			let mut punished = false;
			for _ in 0..2 {
				match handle.recv().await {
					AllMessages::NetworkBridge(
						NetworkBridgeMessage::SendRequests(
							mut reqs, IfDisconnected::ImmediateError
						)
					) => {
						let reqs = reqs.pop().unwrap();
						let outgoing = match reqs {
							Requests::StatementFetching(outgoing) => outgoing,
							_ => panic!("Unexpected request"),
						};
						let req = outgoing.payload;
						assert_eq!(req.relay_parent, metadata.relay_parent);
						assert_eq!(req.candidate_hash, metadata.candidate_hash);
						assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
						// Just drop request - should trigger error.
						requested = true;
					}

					AllMessages::NetworkBridge(
						NetworkBridgeMessage::ReportPeer(p, r)
					) if p == peer_a && r == COST_APPARENT_FLOOD => {
						punished = true;
					}

					m => panic!("Unexpected message: {:?}", m),
				}
			}
			assert!(requested, "large data has not been requested.");
			assert!(punished, "Peer should have been punished for flooding.");

			handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::pin_mut!(test_fut);
		futures::pin_mut!(bg);

		executor::block_on(future::join(test_fut, bg));
	}

	fn make_session_info(validators: Vec<Pair>, groups: Vec<Vec<u32>>) -> SessionInfo {

		let validator_groups: Vec<Vec<ValidatorIndex>> = groups
			.iter().map(|g| g.into_iter().map(|v| ValidatorIndex(*v)).collect()).collect();

		SessionInfo {
			discovery_keys: validators.iter().map(|k| k.public().into()).collect(),
			// Not used:
			n_cores: validator_groups.len() as u32,
			validator_groups,
			validators: validators.iter().map(|k| k.public().into()).collect(),
			// Not used values:
			assignment_keys: Vec::new(),
			zeroth_delay_tranche_width: 0,
			relay_vrf_modulo_samples: 0,
			n_delay_tranches: 0,
			no_show_slots: 0,
			needed_approvals: 0,
		}
	}

	pub fn make_ferdie_keystore() -> SyncCryptoStorePtr {
		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&Sr25519Keyring::Ferdie.to_seed()),
			)
			.expect("Insert key into keystore");
		keystore
	}
}
