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

//! Vote import logic.
//!
//! This module encapsulates the actual logic for importing new votes and provides easy access of
//! the current state for votes for a particular candidate.
//!
//! In particular there is `CandidateVoteState` which tells what can be concluded for a particular set of
//! votes. E.g. whether a dispute is ongoing, whether it is confirmed, concluded, ..
//!
//! Then there is `ImportResult` which reveals information about what changed once additional votes
//! got imported on top of an existing `CandidateVoteState` and reveals "dynamic" information, like whether
//! due to the import a dispute was raised/got confirmed, ...

use std::collections::{BTreeMap, HashMap, HashSet};

use polkadot_node_primitives::{
	disputes::ValidCandidateVotes, CandidateVotes, DisputeStatus, SignedDisputeStatement, Timestamp,
};
use polkadot_node_subsystem_util::rolling_session_window::RollingSessionWindow;
use polkadot_primitives::v2::{
	CandidateReceipt, DisputeStatement, IndexedVec, SessionIndex, SessionInfo,
	ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorPair, ValidatorSignature,
};
use sc_keystore::LocalKeystore;

use crate::LOG_TARGET;

/// (Session) environment of a candidate.
pub struct CandidateEnvironment<'a> {
	/// The session the candidate appeared in.
	session_index: SessionIndex,
	/// Session for above index.
	session: &'a SessionInfo,
	/// Validator indices controlled by this node.
	controlled_indices: HashSet<ValidatorIndex>,
}

impl<'a> CandidateEnvironment<'a> {
	/// Create `CandidateEnvironment`.
	///
	/// Return: `None` in case session is outside of session window.
	pub fn new(
		keystore: &LocalKeystore,
		session_window: &'a RollingSessionWindow,
		session_index: SessionIndex,
	) -> Option<Self> {
		let session = session_window.session_info(session_index)?;
		let controlled_indices = find_controlled_validator_indices(keystore, &session.validators);
		Some(Self { session_index, session, controlled_indices })
	}

	/// Validators in the candidate's session.
	pub fn validators(&self) -> &IndexedVec<ValidatorIndex, ValidatorId> {
		&self.session.validators
	}

	/// `SessionInfo` for the candidate's session.
	pub fn session_info(&self) -> &SessionInfo {
		&self.session
	}

	/// Retrieve `SessionIndex` for this environment.
	pub fn session_index(&self) -> SessionIndex {
		self.session_index
	}

	/// Indices controlled by this node.
	pub fn controlled_indices(&'a self) -> &'a HashSet<ValidatorIndex> {
		&self.controlled_indices
	}
}

/// Whether or not we already issued some statement about a candidate.
pub enum OwnVoteState {
	/// We already voted/issued a statement for the candidate.
	Voted,
	/// We already voted/issued a statement for the candidate and it was an approval vote.
	///
	/// Needs special treatment as we have to make sure to propagate it to peers, to guarantee the
	/// dispute can conclude.
	VotedApproval(Vec<(ValidatorIndex, ValidatorSignature)>),
	/// We not yet voted for the dispute.
	NoVote,
}

impl OwnVoteState {
	fn new<'a>(votes: &CandidateVotes, env: &CandidateEnvironment<'a>) -> Self {
		let mut our_valid_votes = env
			.controlled_indices()
			.iter()
			.filter_map(|i| votes.valid.raw().get_key_value(i))
			.peekable();
		let mut our_invalid_votes =
			env.controlled_indices.iter().filter_map(|i| votes.invalid.get_key_value(i));
		let has_valid_votes = our_valid_votes.peek().is_some();
		let has_invalid_votes = our_invalid_votes.next().is_some();
		let our_approval_votes: Vec<_> = our_valid_votes
			.filter_map(|(index, (k, sig))| {
				if let ValidDisputeStatementKind::ApprovalChecking = k {
					Some((*index, sig.clone()))
				} else {
					None
				}
			})
			.collect();

		if !our_approval_votes.is_empty() {
			return Self::VotedApproval(our_approval_votes)
		}
		if has_valid_votes || has_invalid_votes {
			return Self::Voted
		}
		Self::NoVote
	}

	/// Whether or not we issued a statement for the candidate already.
	fn voted(&self) -> bool {
		match self {
			Self::Voted | Self::VotedApproval(_) => true,
			Self::NoVote => false,
		}
	}

	/// Get own approval votes, if any.
	fn approval_votes(&self) -> Option<&Vec<(ValidatorIndex, ValidatorSignature)>> {
		match self {
			Self::VotedApproval(votes) => Some(&votes),
			_ => None,
		}
	}
}

/// Complete state of votes for a candidate.
///
/// All votes + information whether a dispute is ongoing, confirmed, concluded, whether we already
/// voted, ...
pub struct CandidateVoteState<Votes> {
	/// Votes already existing for the candidate + receipt.
	votes: Votes,

	/// Information about own votes:
	own_vote: OwnVoteState,

	/// Current dispute status, if there is any.
	dispute_status: Option<DisputeStatus>,
}

impl CandidateVoteState<CandidateVotes> {
	/// Create an empty `CandidateVoteState`
	///
	/// in case there have not been any previous votes.
	pub fn new_from_receipt(candidate_receipt: CandidateReceipt) -> Self {
		let votes = CandidateVotes {
			candidate_receipt,
			valid: ValidCandidateVotes::new(),
			invalid: BTreeMap::new(),
		};
		Self { votes, own_vote: OwnVoteState::NoVote, dispute_status: None }
	}

	/// Create a new `CandidateVoteState` from already existing votes.
	pub fn new(votes: CandidateVotes, env: &CandidateEnvironment, now: Timestamp) -> Self {
		let own_vote = OwnVoteState::new(&votes, env);

		let n_validators = env.validators().len();

		let supermajority_threshold =
			polkadot_primitives::v2::supermajority_threshold(n_validators);

		// We have a dispute, if we have votes on both sides:
		let is_disputed = !votes.invalid.is_empty() && !votes.valid.raw().is_empty();

		let dispute_status = if is_disputed {
			let mut status = DisputeStatus::active();
			let byzantine_threshold = polkadot_primitives::v2::byzantine_threshold(n_validators);
			let is_confirmed = votes.voted_indices().len() > byzantine_threshold;
			if is_confirmed {
				status = status.confirm();
			};
			let concluded_for = votes.valid.raw().len() >= supermajority_threshold;
			if concluded_for {
				status = status.conclude_for(now);
			};

			let concluded_against = votes.invalid.len() >= supermajority_threshold;
			if concluded_against {
				status = status.conclude_against(now);
			};
			Some(status)
		} else {
			None
		};

		Self { votes, own_vote, dispute_status }
	}

	/// Import fresh statements.
	///
	/// Result will be a new state plus information about things that changed due to the import.
	pub fn import_statements(
		self,
		env: &CandidateEnvironment,
		statements: Vec<(SignedDisputeStatement, ValidatorIndex)>,
		now: Timestamp,
	) -> ImportResult {
		let (mut votes, old_state) = self.into_old_state();

		let mut new_invalid_voters = Vec::new();
		let mut imported_invalid_votes = 0;
		let mut imported_valid_votes = 0;

		let expected_candidate_hash = votes.candidate_receipt.hash();

		for (statement, val_index) in statements {
			if env
				.validators()
				.get(val_index)
				.map_or(true, |v| v != statement.validator_public())
			{
				gum::error!(
					target: LOG_TARGET,
					?val_index,
					session= ?env.session_index,
					claimed_key = ?statement.validator_public(),
					"Validator index doesn't match claimed key",
				);

				continue
			}
			if statement.candidate_hash() != &expected_candidate_hash {
				gum::error!(
					target: LOG_TARGET,
					?val_index,
					session= ?env.session_index,
					given_candidate_hash = ?statement.candidate_hash(),
					?expected_candidate_hash,
					"Vote is for unexpected candidate!",
				);
				continue
			}
			if statement.session_index() != env.session_index() {
				gum::error!(
					target: LOG_TARGET,
					?val_index,
					session= ?env.session_index,
					given_candidate_hash = ?statement.candidate_hash(),
					?expected_candidate_hash,
					"Vote is for unexpected session!",
				);
				continue
			}

			match statement.statement() {
				DisputeStatement::Valid(valid_kind) => {
					let fresh = votes.valid.insert_vote(
						val_index,
						*valid_kind,
						statement.into_validator_signature(),
					);
					if fresh {
						imported_valid_votes += 1;
					}
				},
				DisputeStatement::Invalid(invalid_kind) => {
					let fresh = votes
						.invalid
						.insert(val_index, (*invalid_kind, statement.into_validator_signature()))
						.is_none();
					if fresh {
						new_invalid_voters.push(val_index);
						imported_invalid_votes += 1;
					}
				},
			}
		}

		let new_state = Self::new(votes, env, now);

		ImportResult {
			old_state,
			new_state,
			imported_invalid_votes,
			imported_valid_votes,
			imported_approval_votes: 0,
			new_invalid_voters,
		}
	}

	/// Retrieve `CandidateReceipt` in `CandidateVotes`.
	pub fn candidate_receipt(&self) -> &CandidateReceipt {
		&self.votes.candidate_receipt
	}

	/// Extract `CandidateVotes` for handling import of new statements.
	fn into_old_state(self) -> (CandidateVotes, CandidateVoteState<()>) {
		let CandidateVoteState { votes, own_vote, dispute_status } = self;
		(votes, CandidateVoteState { votes: (), own_vote, dispute_status })
	}
}

impl<V> CandidateVoteState<V> {
	/// Whether or not we have an ongoing dispute.
	pub fn is_disputed(&self) -> bool {
		self.dispute_status.is_some()
	}

	/// Whether there is an ongoing confirmed dispute.
	///
	/// This checks whether there is a dispute ongoing and we have more than byzantine threshold
	/// votes.
	pub fn is_confirmed(&self) -> bool {
		self.dispute_status.map_or(false, |s| s.is_confirmed_concluded())
	}

	/// This machine already cast some vote in that dispute/for that candidate.
	pub fn has_own_vote(&self) -> bool {
		self.own_vote.voted()
	}

	/// Own approval votes if any:
	pub fn own_approval_votes(&self) -> Option<&Vec<(ValidatorIndex, ValidatorSignature)>> {
		self.own_vote.approval_votes()
	}

	/// Whether or not there is a dispute and it has already enough valid votes to conclude.
	pub fn has_concluded_for(&self) -> bool {
		self.dispute_status.map_or(false, |s| s.has_concluded_for())
	}

	/// Whether or not there is a dispute and it has already enough invalid votes to conclude.
	pub fn has_concluded_against(&self) -> bool {
		self.dispute_status.map_or(false, |s| s.has_concluded_against())
	}

	/// Get access to the dispute status, in case there is one.
	pub fn dispute_status(&self) -> &Option<DisputeStatus> {
		&self.dispute_status
	}

	/// Access to underlying votes.
	pub fn votes(&self) -> &V {
		&self.votes
	}
}

/// An ongoing statement/vote import.
pub struct ImportResult {
	/// The state we had before importing new statements.
	old_state: CandidateVoteState<()>,
	/// The new state after importing the new statements.
	new_state: CandidateVoteState<CandidateVotes>,
	/// New invalid voters as of this import.
	new_invalid_voters: Vec<ValidatorIndex>,
	/// Number of successfully imported valid votes.
	imported_invalid_votes: u32,
	/// Number of successfully imported invalid votes.
	imported_valid_votes: u32,
	/// Number of approval votes imported via `import_approval_votes()`.
	///
	/// And only those: If normal import included approval votes, those are not counted here.
	///
	/// In other words, without a call `import_approval_votes()` this will always be 0.
	imported_approval_votes: u32,
}

impl ImportResult {
	/// Whether or not anything has changed due to the import.
	pub fn votes_changed(&self) -> bool {
		self.imported_valid_votes != 0 || self.imported_invalid_votes != 0
	}

	/// The dispute state has changed in some way.
	///
	/// - freshly disputed
	/// - freshly confirmed
	/// - freshly concluded (valid or invalid)
	pub fn dispute_state_changed(&self) -> bool {
		self.is_freshly_disputed() || self.is_freshly_confirmed() || self.is_freshly_concluded()
	}

	/// State as it was before import.
	pub fn old_state(&self) -> &CandidateVoteState<()> {
		&self.old_state
	}

	/// State after import
	pub fn new_state(&self) -> &CandidateVoteState<CandidateVotes> {
		&self.new_state
	}

	/// New "invalid" voters encountered during import.
	pub fn new_invalid_voters(&self) -> &Vec<ValidatorIndex> {
		&self.new_invalid_voters
	}

	/// Number of imported valid votes.
	pub fn imported_valid_votes(&self) -> u32 {
		self.imported_valid_votes
	}

	/// Number of imported invalid votes.
	pub fn imported_invalid_votes(&self) -> u32 {
		self.imported_invalid_votes
	}

	/// Number of imported approval votes.
	pub fn imported_approval_votes(&self) -> u32 {
		self.imported_approval_votes
	}

	/// Whether we now have a dispute and did not prior to the import.
	pub fn is_freshly_disputed(&self) -> bool {
		!self.old_state().is_disputed() && self.new_state().is_disputed()
	}

	/// Whether we just surpassed the byzantine threshold.
	pub fn is_freshly_confirmed(&self) -> bool {
		!self.old_state().is_confirmed() && self.new_state().is_confirmed()
	}

	/// Whether or not any dispute just concluded valid due to the import.
	pub fn is_freshly_concluded_for(&self) -> bool {
		!self.old_state().has_concluded_for() && self.new_state().has_concluded_for()
	}

	/// Whether or not any dispute just concluded invalid due to the import.
	pub fn is_freshly_concluded_against(&self) -> bool {
		!self.old_state().has_concluded_against() && self.new_state().has_concluded_against()
	}

	/// Whether or not any dispute just concluded either invalid or valid due to the import.
	pub fn is_freshly_concluded(&self) -> bool {
		self.is_freshly_concluded_against() || self.is_freshly_concluded_for()
	}

	/// Modify this `ImportResult`s, by importing additional approval votes.
	///
	/// Both results and `new_state` will be changed as if those approval votes had been in the
	/// original import.
	pub fn import_approval_votes(
		self,
		env: &CandidateEnvironment,
		approval_votes: HashMap<ValidatorIndex, ValidatorSignature>,
		now: Timestamp,
	) -> Self {
		let Self {
			old_state,
			new_state,
			new_invalid_voters,
			mut imported_valid_votes,
			imported_invalid_votes,
			mut imported_approval_votes,
		} = self;

		let (mut votes, _) = new_state.into_old_state();

		for (index, sig) in approval_votes.into_iter() {
			debug_assert!(
				{
					let pub_key = &env.session_info().validators.get(index).expect("indices are validated by approval-voting subsystem; qed");
					let candidate_hash = votes.candidate_receipt.hash();
					let session_index = env.session_index();
					DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking)
						.check_signature(pub_key, candidate_hash, session_index, &sig)
						.is_ok()
				},
				"Signature check for imported approval votes failed! This is a serious bug. Session: {:?}, candidate hash: {:?}, validator index: {:?}", env.session_index(), votes.candidate_receipt.hash(), index
			);
			if votes.valid.insert_vote(index, ValidDisputeStatementKind::ApprovalChecking, sig) {
				imported_valid_votes += 1;
				imported_approval_votes += 1;
			}
		}

		let new_state = CandidateVoteState::new(votes, env, now);

		Self {
			old_state,
			new_state,
			new_invalid_voters,
			imported_valid_votes,
			imported_invalid_votes,
			imported_approval_votes,
		}
	}

	/// All done, give me those votes.
	///
	/// Returns: `None` in case nothing has changed (import was redundant).
	pub fn into_updated_votes(self) -> Option<CandidateVotes> {
		if self.votes_changed() {
			let CandidateVoteState { votes, .. } = self.new_state;
			Some(votes)
		} else {
			None
		}
	}
}

/// Find indices controlled by this validator.
///
/// That is all `ValidatorIndex`es we have private keys for. Usually this will only be one.
fn find_controlled_validator_indices(
	keystore: &LocalKeystore,
	validators: &IndexedVec<ValidatorIndex, ValidatorId>,
) -> HashSet<ValidatorIndex> {
	let mut controlled = HashSet::new();
	for (index, validator) in validators.iter().enumerate() {
		if keystore.key_pair::<ValidatorPair>(validator).ok().flatten().is_none() {
			continue
		}

		controlled.insert(ValidatorIndex(index as _));
	}

	controlled
}
