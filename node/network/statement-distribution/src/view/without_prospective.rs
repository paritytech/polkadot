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

use futures::{
	channel::{mpsc, oneshot},
	future::RemoteHandle,
	prelude::*,
};
use indexmap::IndexMap;

use polkadot_node_network_protocol::{
	self as net_protocol, PeerId, UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem::Span;
use polkadot_node_subsystem_util::backing_implicit_view::View as ImplicitView;
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, Id as ParaId,
	PersistedValidationData, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
	ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use crate::LOG_TARGET;
use super::{StatementFingerprint, StoredStatement, StoredStatementComparator};

/// The maximum amount of candidates each validator is allowed to second at any relay-parent.
/// Short for "Validator Candidate Threshold".
///
/// This is the amount of candidates we keep per validator at any relay-parent.
/// Typically we will only keep 1, but when a validator equivocates we will need to track 2.
const VC_THRESHOLD: usize = 2;

#[derive(Debug, PartialEq, Eq)]
enum DeniedStatement {
	NotUseful,
	UsefulButKnown,
}

/// The view of the protocol state for all relay-parents where prospective
/// parachains aren't enabled.
///
/// Prior to prospective parachains, the only relay-parents that were allowed
/// were the active leaves directly, and data couldn't be reused across active leaves.
///
/// Therefore, this is a simple mapping from active leaf hashes to
/// self-contained data relating to those hashes.
pub(crate) struct View  {
	per_relay_parent: HashMap<Hash, RelayParentInfo>,
}

impl View {
	/// Whether the view contains a given relay-parent.
	pub(crate) fn contains(&self, leaf_hash: &Hash) -> bool {
		self.per_relay_parent.contains_key(leaf_hash)
	}

	/// Deactivate the given leaf in the view, if it exists, and
	/// clean up after it.
	pub(crate) fn deactivate_leaf(&mut self, leaf_hash: &Hash) {
		let _ = self.per_relay_parent.remove(leaf_hash);
	}

	/// Activate the given relay-parent in the view. This overwrites
	/// any existing entry, and should only be called for fresh leaves.
	pub(crate) fn activate_leaf(
		&mut self,
		leaf_hash: Hash,
		relay_parent_info: RelayParentInfo,
	) {
		let _ = self.per_relay_parent.insert(leaf_hash, relay_parent_info);
	}
}

/// Info about a fetch in progress under the v2 protocol.
struct FetchingInfo {
	/// All peers that send us a `LargeStatement` or a `Valid` statement for the given
	/// `CandidateHash`, together with their originally sent messages.
	///
	/// We use an `IndexMap` here to preserve the ordering of peers sending us messages. This is
	/// desirable because we reward first sending peers with reputation.
	available_peers: IndexMap<PeerId, Vec<net_protocol::StatementDistributionMessage>>,
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

/// Large statement fetching status.
enum LargeStatementStatus {
	/// We are currently fetching the statement data from a remote peer. We keep a list of other nodes
	/// claiming to have that data and will fallback on them.
	Fetching(FetchingInfo),
	/// Statement data is fetched or we got it locally via `StatementDistributionMessage::Share`.
	FetchedOrShared(CommittedCandidateReceipt),
}

/// The local node's information regarding statements under a relay-parent without
/// prospective parachains.
pub struct RelayParentInfo {
	/// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<CandidateHash>,
	/// Stored statements for circulation to peers.
	///
	/// These are iterable in insertion order, and `Seconded` statements are always
	/// accepted before dependent statements.
	statements: IndexMap<StoredStatementComparator, SignedFullStatement>,
	/// Large statements we are waiting for with associated meta data.
	waiting_large_statements: HashMap<CandidateHash, LargeStatementStatus>,
	/// The parachain validators at the head's child session index.
	validators: Vec<ValidatorId>,
	/// The current session index of this fork.
	session_index: sp_staking::SessionIndex,
	/// How many `Seconded` statements we've seen per validator.
	seconded_counts: HashMap<ValidatorIndex, usize>,
	/// A cache for `PersistedValidationData` at this relay-parent
	/// by para-id.
	valid_pvds: HashMap<ParaId, PersistedValidationData>,
	/// A Jaeger span for this head, so we can attach data to it.
	span: Span,
}

#[derive(Debug)]
enum NotedStatement<'a> {
	NotUseful,
	Fresh(StoredStatement<'a>),
	UsefulButKnown,
}

impl RelayParentInfo {
	pub(super) fn new(
		validators: Vec<ValidatorId>,
		session_index: sp_staking::SessionIndex,
		valid_pvds: HashMap<ParaId, PersistedValidationData>,
		span: Span,
	) -> Self {
		RelayParentInfo {
			candidates: Default::default(),
			statements: Default::default(),
			waiting_large_statements: Default::default(),
			validators,
			session_index,
			seconded_counts: Default::default(),
			valid_pvds,
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
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Extra statement is ignored"
					);
					return NotedStatement::NotUseful
				}

				self.candidates.insert(h);
				if let Some(old) = self.statements.insert(comparator.clone(), statement) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?old,
						"Known statement"
					);
					NotedStatement::UsefulButKnown
				} else {
					*seconded_so_far += 1;

					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?self.statements.last().expect("Just inserted").1,
						"Noted new statement"
					);
					// This will always return `Some` because it was just inserted.
					let key_value = self
						.statements
						.get_key_value(&comparator)
						.expect("Statement was just inserted; qed");

					NotedStatement::Fresh(key_value.into())
				}
			},
			CompactStatement::Valid(h) => {
				if !self.candidates.contains(&h) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Statement for unknown candidate"
					);
					return NotedStatement::NotUseful
				}

				if let Some(old) = self.statements.insert(comparator.clone(), statement) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						statement = ?old,
						"Known statement"
					);
					NotedStatement::UsefulButKnown
				} else {
					gum::trace!(
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
							.into(),
					)
				}
			},
		}
	}

	/// Returns an error if the statement is already known or not useful
	/// without modifying the internal state.
	fn check_useful_or_unknown(
		&self,
		statement: &UncheckedSignedStatement,
	) -> std::result::Result<(), DeniedStatement> {
		let validator_index = statement.unchecked_validator_index();
		let compact = statement.unchecked_payload();
		let comparator = StoredStatementComparator {
			compact: compact.clone(),
			validator_index,
			signature: statement.unchecked_signature().clone(),
		};

		match compact {
			CompactStatement::Seconded(_) => {
				let seconded_so_far = self.seconded_counts.get(&validator_index).unwrap_or(&0);
				if *seconded_so_far >= VC_THRESHOLD {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Extra statement is ignored",
					);
					return Err(DeniedStatement::NotUseful)
				}

				if self.statements.contains_key(&comparator) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Known statement",
					);
					return Err(DeniedStatement::UsefulButKnown)
				}
			},
			CompactStatement::Valid(h) => {
				if !self.candidates.contains(&h) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Statement for unknown candidate",
					);
					return Err(DeniedStatement::NotUseful)
				}

				if self.statements.contains_key(&comparator) {
					gum::trace!(
						target: LOG_TARGET,
						?validator_index,
						?statement,
						"Known statement",
					);
					return Err(DeniedStatement::UsefulButKnown)
				}
			},
		}
		Ok(())
	}

	/// Get an iterator over all statements for the active head. Seconded statements come first.
	fn statements(&self) -> impl Iterator<Item = StoredStatement<'_>> + '_ {
		self.statements.iter().map(Into::into)
	}

	/// Get an iterator over all statements for the active head that are for a particular candidate.
	fn statements_about(
		&self,
		candidate_hash: CandidateHash,
	) -> impl Iterator<Item = StoredStatement<'_>> + '_ {
		self.statements()
			.filter(move |s| s.compact().candidate_hash() == &candidate_hash)
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
			gum::warn!(
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
		!self.remote_observed.contains(h) && !self.remote_observed.is_full()
	}
}

fn note_hash(
	observed: &mut arrayvec::ArrayVec<[CandidateHash; VC_THRESHOLD]>,
	h: CandidateHash,
) -> bool {
	if observed.contains(&h) {
		return true
	}

	observed.try_push(h).is_ok()
}

/// A peer's view of the relay-chain state for relay-parents that
/// don't support prospective parachains.
#[derive(Default)]
pub(crate) struct PeerView {
	per_relay_parent: HashMap<Hash, PeerRelayParentKnowledge>,
}

impl PeerView {
	/// Updates our view of the peer's knowledge with this statement's fingerprint based
	/// on something that we would like to send to the peer.
	///
	/// NOTE: assumes `self.can_send` returned true before this call.
	///
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	///
	/// This returns `true` if this is the first time the peer has become aware of a
	/// candidate with the given hash.
	fn send(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &StatementFingerprint,
	) -> bool {
		debug_assert!(
			self.can_send(relay_parent, fingerprint),
			"send is only called after `can_send` returns true; qed",
		);
		self.per_relay_parent
			.get_mut(relay_parent)
			.expect("send is only called after `can_send` returns true; qed")
			.send(fingerprint)
	}

	/// This returns `None` if the peer cannot accept this statement, without altering internal
	/// state.
	fn can_send(
		&self,
		relay_parent: &Hash,
		fingerprint: &StatementFingerprint,
	) -> bool {
		self.per_relay_parent.get(relay_parent).map_or(false, |k| k.can_send(fingerprint))
	}

	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based on
	/// a message we are receiving from the peer.
	///
	/// Provide the maximum message count that we can receive per candidate. In practice we should
	/// not receive more statements for any one candidate than there are members in the group assigned
	/// to that para, but this maximum needs to be lenient to account for equivocations that may be
	/// cross-group. As such, a maximum of 2 * `n_validators` is recommended.
	///
	/// This returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	///
	/// If this returns `Ok`, the internal state has been altered. After `receive`ing a new
	/// candidate, we are then cleared to send the peer further statements about that candidate.
	///
	/// This returns `Ok(true)` if this is the first time the peer has become aware of a
	/// candidate with given hash.
	fn receive(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &StatementFingerprint,
		max_message_count: usize,
	) -> std::result::Result<bool, Rep> {
		self.per_relay_parent
			.get_mut(relay_parent)
			.ok_or(crate::COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE)?
			.receive(fingerprint, max_message_count)
	}

	/// This method does the same checks as `receive` without modifying the internal state.
	/// Returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	fn check_can_receive(
		&self,
		relay_parent: &Hash,
		fingerprint: &StatementFingerprint,
		max_message_count: usize,
	) -> std::result::Result<(), Rep> {
		self.per_relay_parent
			.get(relay_parent)
			.ok_or(crate::COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE)?
			.check_can_receive(fingerprint, max_message_count)
	}

	/// Receive a notice about out of view statement and returns the value of the old flag
	fn receive_unexpected(&mut self, relay_parent: &Hash) -> usize {
		self.per_relay_parent
			.get_mut(relay_parent)
			.map_or(0_usize, |relay_parent_peer_knowledge| {
				let old = relay_parent_peer_knowledge.unexpected_count;
				relay_parent_peer_knowledge.unexpected_count += 1_usize;
				old
			})
	}

	/// Basic flood protection for large statements.
	fn receive_large_statement(&mut self, relay_parent: &Hash) -> std::result::Result<(), Rep> {
		self.per_relay_parent
			.get_mut(relay_parent)
			.ok_or(crate::COST_UNEXPECTED_STATEMENT_MISSING_KNOWLEDGE)?
			.receive_large_statement()
	}
}

/// A peer's knowledge of statements in some relay-parent which doesn't
/// support prospective parachains.
#[derive(Default)]
pub(crate) struct PeerRelayParentKnowledge {
	/// candidates that the peer is aware of because we sent statements to it. This indicates that we can
	/// send other statements pertaining to that candidate.
	sent_candidates: HashSet<CandidateHash>,
	/// candidates that peer is aware of, because we received statements from it.
	received_candidates: HashSet<CandidateHash>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to the peer by us.
	sent_statements: HashSet<StatementFingerprint>,
	/// fingerprints of all statements a peer should be aware of: those that
	/// were sent to us by the peer.
	received_statements: HashSet<StatementFingerprint>,
	/// How many candidates this peer is aware of for each given validator index.
	seconded_counts: HashMap<ValidatorIndex, VcPerPeerTracker>,
	/// How many statements we've received for each candidate that we're aware of.
	received_message_count: HashMap<CandidateHash, usize>,

	/// How many large statements this peer already sent us.
	///
	/// Flood protection for large statements is rather hard and as soon as we get
	/// `https://github.com/paritytech/polkadot/issues/2979` implemented also no longer necessary.
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

	/// We have seen a message that that is unexpected from this peer, so note this fact
	/// and stop subsequent logging and peer reputation flood.
	unexpected_count: usize,
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
	fn send(&mut self, fingerprint: &StatementFingerprint) -> bool {
		debug_assert!(
			self.can_send(fingerprint),
			"send is only called after `can_send` returns true; qed",
		);

		let new_known = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				self.seconded_counts.entry(fingerprint.1).or_default().note_local(h.clone());

				let was_known = self.is_known_candidate(h);
				self.sent_candidates.insert(h.clone());
				!was_known
			},
			CompactStatement::Valid(_) => false,
		};

		self.sent_statements.insert(fingerprint.clone());

		new_known
	}

	/// This returns `true` if the peer cannot accept this statement, without altering internal
	/// state, `false` otherwise.
	fn can_send(&self, fingerprint: &StatementFingerprint) -> bool {
		let already_known = self.sent_statements.contains(fingerprint) ||
			self.received_statements.contains(fingerprint);

		if already_known {
			return false
		}

		match fingerprint.0 {
			CompactStatement::Valid(ref h) => {
				// The peer can only accept Valid statements for which it is aware
				// of the corresponding candidate.
				self.is_known_candidate(h)
			},
			CompactStatement::Seconded(_) => true,
		}
	}

	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint based on
	/// a message we are receiving from the peer.
	///
	/// Provide the maximum message count that we can receive per candidate. In practice we should
	/// not receive more statements for any one candidate than there are members in the group assigned
	/// to that para, but this maximum needs to be lenient to account for equivocations that may be
	/// cross-group. As such, a maximum of 2 * `n_validators` is recommended.
	///
	/// This returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	///
	/// If this returns `Ok`, the internal state has been altered. After `receive`ing a new
	/// candidate, we are then cleared to send the peer further statements about that candidate.
	///
	/// This returns `Ok(true)` if this is the first time the peer has become aware of a
	/// candidate with given hash.
	fn receive(
		&mut self,
		fingerprint: &StatementFingerprint,
		max_message_count: usize,
	) -> std::result::Result<bool, Rep> {
		// We don't check `sent_statements` because a statement could be in-flight from both
		// sides at the same time.
		if self.received_statements.contains(fingerprint) {
			return Err(crate::COST_DUPLICATE_STATEMENT)
		}

		let (candidate_hash, fresh) = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				let allowed_remote = self
					.seconded_counts
					.entry(fingerprint.1)
					.or_insert_with(Default::default)
					.note_remote(h.clone());

				if !allowed_remote {
					return Err(crate::COST_UNEXPECTED_STATEMENT_REMOTE)
				}

				(h, !self.is_known_candidate(h))
			},
			CompactStatement::Valid(ref h) => {
				if !self.is_known_candidate(h) {
					return Err(crate::COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE)
				}

				(h, false)
			},
		};

		{
			let received_per_candidate =
				self.received_message_count.entry(*candidate_hash).or_insert(0);

			if *received_per_candidate >= max_message_count {
				return Err(crate::COST_APPARENT_FLOOD)
			}

			*received_per_candidate += 1;
		}

		self.received_statements.insert(fingerprint.clone());
		self.received_candidates.insert(candidate_hash.clone());
		Ok(fresh)
	}

	/// Note a received large statement metadata.
	fn receive_large_statement(&mut self) -> std::result::Result<(), Rep> {
		if self.large_statement_count >= crate::MAX_LARGE_STATEMENTS_PER_SENDER {
			return Err(crate::COST_APPARENT_FLOOD)
		}
		self.large_statement_count += 1;
		Ok(())
	}

	/// This method does the same checks as `receive` without modifying the internal state.
	/// Returns an error if the peer should not have sent us this message according to protocol
	/// rules for flood protection.
	fn check_can_receive(
		&self,
		fingerprint: &StatementFingerprint,
		max_message_count: usize,
	) -> std::result::Result<(), Rep> {
		// We don't check `sent_statements` because a statement could be in-flight from both
		// sides at the same time.
		if self.received_statements.contains(fingerprint) {
			return Err(crate::COST_DUPLICATE_STATEMENT)
		}

		let candidate_hash = match fingerprint.0 {
			CompactStatement::Seconded(ref h) => {
				let allowed_remote = self
					.seconded_counts
					.get(&fingerprint.1)
					.map_or(true, |r| r.is_wanted_candidate(h));

				if !allowed_remote {
					return Err(crate::COST_UNEXPECTED_STATEMENT_REMOTE)
				}

				h
			},
			CompactStatement::Valid(ref h) => {
				if !self.is_known_candidate(&h) {
					return Err(crate::COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE)
				}

				h
			},
		};

		let received_per_candidate = self.received_message_count.get(candidate_hash).unwrap_or(&0);

		if *received_per_candidate >= max_message_count {
			Err(crate::COST_APPARENT_FLOOD)
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

#[cfg(test)]
mod tests {
	use super::*;

	use polkadot_primitives_test_helpers::{
		dummy_committed_candidate_receipt, dummy_hash, AlwaysZeroRng,
	};
	use polkadot_node_primitives::Statement;
	use polkadot_node_subsystem::jaeger;
	use polkadot_primitives::v2::SigningContext;

	use sc_keystore::LocalKeystore;
	use sp_keyring::Sr25519Keyring;
	use sp_application_crypto::{sr25519::Pair, AppKey, Pair as TraitPair};
	use sp_keystore::{CryptoStore, SyncCryptoStore, SyncCryptoStorePtr};

	use assert_matches::assert_matches;
	use futures::executor::block_on;
	use std::sync::Arc;

	#[test]
	fn active_head_accepts_only_2_seconded_per_validator() {
		let validators = vec![
			Sr25519Keyring::Alice.public().into(),
			Sr25519Keyring::Bob.public().into(),
			Sr25519Keyring::Charlie.public().into(),
		];
		let parent_hash: Hash = [1; 32].into();

		let session_index = 1;
		let signing_context = SigningContext { parent_hash, session_index };

		let candidate_a = {
			let mut c = dummy_committed_candidate_receipt(dummy_hash());
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = ParaId::from(1u32);
			c
		};

		let candidate_b = {
			let mut c = dummy_committed_candidate_receipt(dummy_hash());
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = ParaId::from(2u32);
			c
		};

		let candidate_c = {
			let mut c = dummy_committed_candidate_receipt(dummy_hash());
			c.descriptor.relay_parent = parent_hash;
			c.descriptor.para_id = ParaId::from(3u32);
			c
		};

		let mut head_data = RelayParentInfo::new(
			validators,
			session_index,
			HashMap::new(),
			jaeger::Span::Disabled,
		);

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
		let alice_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&Sr25519Keyring::Alice.to_seed()),
		)
		.unwrap();
		let bob_public = SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&Sr25519Keyring::Bob.to_seed()),
		)
		.unwrap();

		// note A
		let a_seconded_val_0 = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_a.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(head_data
			.check_useful_or_unknown(&a_seconded_val_0.clone().convert_payload().into())
			.is_ok());
		let noted = head_data.note_statement(a_seconded_val_0.clone());

		assert_matches!(noted, NotedStatement::Fresh(_));

		// note A (duplicate)
		assert_eq!(
			head_data.check_useful_or_unknown(&a_seconded_val_0.clone().convert_payload().into()),
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
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(head_data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
		let noted = head_data.note_statement(statement);
		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (beyond 2 - ignored)
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert_eq!(
			head_data.check_useful_or_unknown(&statement.clone().convert_payload().into()),
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
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(head_data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
		let noted = head_data.note_statement(statement);
		assert_matches!(noted, NotedStatement::Fresh(_));

		// note C (new validator)
		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate_c.clone()),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(head_data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
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
		assert!(knowledge
			.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
			.is_ok());
		assert!(knowledge
			.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
			.unwrap());
		assert!(!knowledge.can_send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0))));
	}

	#[test]
	fn per_peer_relay_parent_knowledge_receive() {
		let mut knowledge = PeerRelayParentKnowledge::default();

		let hash_a = CandidateHash([1; 32].into());

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
			Err(crate::COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
			Err(crate::COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE),
		);

		assert!(knowledge
			.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
			.is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3),
			Ok(true),
		);

		// Push statements up to the flood limit.
		assert!(knowledge
			.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3)
			.is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3),
			Ok(false),
		);

		assert!(knowledge.is_known_candidate(&hash_a));
		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 2);

		assert!(knowledge
			.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3)
			.is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Ok(false),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
			Err(crate::COST_APPARENT_FLOOD),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
			Err(crate::COST_APPARENT_FLOOD),
		);

		assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);
		assert_eq!(knowledge.received_statements.len(), 3); // number of prior `Ok`s.

		// Now make sure that the seconding limit is respected.
		let hash_b = CandidateHash([2; 32].into());
		let hash_c = CandidateHash([3; 32].into());

		assert!(knowledge
			.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3)
			.is_ok());
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
			Ok(true),
		);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
			Err(crate::COST_UNEXPECTED_STATEMENT_REMOTE),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
			Err(crate::COST_UNEXPECTED_STATEMENT_REMOTE),
		);

		// Last, make sure that already-known statements are disregarded.
		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Err(crate::COST_DUPLICATE_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
			Err(crate::COST_DUPLICATE_STATEMENT),
		);

		assert_eq!(
			knowledge.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
			Err(crate::COST_DUPLICATE_STATEMENT),
		);
		assert_eq!(
			knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
			Err(crate::COST_DUPLICATE_STATEMENT),
		);
	}
}
