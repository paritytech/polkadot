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

//! Handles the local node's view of the relay-chain state, allowed relay
//! parents of active leaves, and implements a message store for all known
//! statements.

use futures::{
	channel::{mpsc, oneshot},
	future::RemoteHandle,
	prelude::*,
};
use indexmap::IndexMap;

use polkadot_node_network_protocol::{self as net_protocol, PeerId};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem::PerLeafSpan;
use polkadot_node_subsystem_util::backing_implicit_view::View as ImplicitView;
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, UncheckedSignedStatement,
	ValidatorId, ValidatorIndex, ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use super::{LOG_TARGET, VC_THRESHOLD};

/// The local node's view of the protocol state and messages.
pub struct View {
	implicit_view: ImplicitView,
	per_leaf: HashMap<Hash, LeafData>,
	/// State tracked for all relay-parents backing work is ongoing for. This includes
	/// all active leaves.
	///
	/// relay-parents fall into one of 3 categories.
	///   1. active leaves which do support prospective parachains
	///   2. active leaves which do not support prospective parachains
	///   3. relay-chain blocks which are ancestors of an active leaf and
	///      do support prospective parachains.
	///
	/// Relay-chain blocks which don't support prospective parachains are
	/// never included in the fragment trees of active leaves which do.
	///
	/// While it would be technically possible to support such leaves in
	/// fragment trees, it only benefits the transition period when asynchronous
	/// backing is being enabled and complicates code complexity.
	per_relay_parent: HashMap<Hash, RelayParentData>,
}

/// Whether a leaf has prospective parachains enabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProspectiveParachainsMode {
	/// Prospective parachains are enabled at the leaf.
	Enabled,
	/// Prospective parachains are disabled at the leaf.
	Disabled,
}

struct LeafData {
	mode: ProspectiveParachainsMode,
}

enum RelayParentData {
	VStaging(RelayParentWithProspective),
	V2(RelayParentWithoutProspective),
}

#[derive(Debug, PartialEq, Eq)]
enum DeniedStatement {
	NotUseful,
	UsefulButKnown,
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
	fn from(
		(comparator, statement): (&'a StoredStatementComparator, &'a SignedFullStatement),
	) -> Self {
		Self { comparator, statement }
	}
}

// A statement stored while a relay chain head is active.
#[derive(Debug, Copy, Clone)]
struct StoredStatement<'a> {
	comparator: &'a StoredStatementComparator,
	statement: &'a SignedFullStatement,
}

impl<'a> StoredStatement<'a> {
	fn compact(&self) -> &'a CompactStatement {
		&self.comparator.compact
	}

	fn fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
		(self.comparator.compact.clone(), self.statement.validator_index())
	}
}

struct RelayParentWithProspective;

/// Info about a fetch in progress.
struct V2FetchingInfo {
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
enum LargeStatementStatusWithoutProspective {
	/// We are currently fetching the statement data from a remote peer. We keep a list of other nodes
	/// claiming to have that data and will fallback on them.
	Fetching(V2FetchingInfo),
	/// Statement data is fetched or we got it locally via `StatementDistributionMessage::Share`.
	FetchedOrShared(CommittedCandidateReceipt),
}

struct RelayParentWithoutProspective {
	/// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<CandidateHash>,
	/// Stored statements for circulation to peers.
	///
	/// These are iterable in insertion order, and `Seconded` statements are always
	/// accepted before dependent statements.
	statements: IndexMap<StoredStatementComparator, SignedFullStatement>,
	/// Large statements we are waiting for with associated meta data.
	waiting_large_statements: HashMap<CandidateHash, LargeStatementStatusWithoutProspective>,
	/// The parachain validators at the head's child session index.
	validators: Vec<ValidatorId>,
	/// The current session index of this fork.
	session_index: sp_staking::SessionIndex,
	/// How many `Seconded` statements we've seen per validator.
	seconded_counts: HashMap<ValidatorIndex, usize>,
	/// A Jaeger span for this head, so we can attach data to it.
	span: PerLeafSpan,
}

#[derive(Debug)]
enum NotedStatement<'a> {
	NotUseful,
	Fresh(StoredStatement<'a>),
	UsefulButKnown,
}

impl RelayParentWithoutProspective {
	fn new(
		validators: Vec<ValidatorId>,
		session_index: sp_staking::SessionIndex,
		span: PerLeafSpan,
	) -> Self {
		RelayParentWithoutProspective {
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
