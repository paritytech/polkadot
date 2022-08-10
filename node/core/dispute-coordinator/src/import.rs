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

use std::collections::{BTreeMap, HashMap, HashSet};

use polkadot_node_primitives::{disputes::ValidVoteData, CandidateVotes, SignedDisputeStatement};
use polkadot_node_subsystem_util::rolling_session_window::RollingSessionWindow;
use polkadot_primitives::v2::{
	CandidateReceipt, DisputeStatement, SessionIndex, SessionInfo, ValidDisputeStatementKind,
	ValidatorId, ValidatorIndex, ValidatorPair, ValidatorSignature,
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
	pub fn validators(&self) -> &Vec<ValidatorId> {
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
	VotedApproval(Vec<ValidVoteData>),
	/// We not yet voted for the dispute.
	NoVote,
}

impl OwnVoteState {
	fn new<'a>(votes: &CandidateVotes, env: &CandidateEnvironment<'a>) -> Self {
		let mut our_valid_votes = env
			.controlled_indices()
			.iter()
			.filter_map(|i| votes.valid.get_key_value(i))
			.peekable();
		let mut our_invalid_votes =
			env.controlled_indices.iter().filter_map(|i| votes.invalid.get_key_value(i));
		let has_valid_votes = our_valid_votes.peek().is_some();
		let has_invalid_votes = our_invalid_votes.next().is_some();
		let our_approval_votes: Vec<_> = our_valid_votes
			.filter(|(_, (k, _))| k == &ValidDisputeStatementKind::ApprovalChecking)
			.map(|(k, v)| (*k, v.clone()))
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
	fn approval_votes(&self) -> Option<&Vec<ValidVoteData>> {
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
pub struct VoteState<Votes> {
	/// Votes already existing for the candidate + receipt.
	votes: Votes,

	/// Information about own votes:
	own_vote: OwnVoteState,

	/// Whether or not the dispute concluded invalid.
	concluded_invalid: bool,

	/// Whether or not the dispute concluded valid.
	///
	/// Note: Due to equivocations it is technically possible for a dispute to conclude both valid
	/// and invalid. In that case the invalid result takes precedence.
	concluded_valid: bool,

	/// There is an ongoing dispute and we reached f+1 votes -> the dispute is confirmed
	///
	/// as at least one honest validator cast a vote for the candidate.
	is_confirmed: bool,

	/// Whether or not we have an ongoing dispute.
	is_disputed: bool,
}

impl VoteState<CandidateVotes> {
	/// Create an empty `VoteState`
	///
	/// in case there have not been any previous votes.
	pub fn new_from_receipt(candidate_receipt: CandidateReceipt) -> Self {
		let votes =
			CandidateVotes { candidate_receipt, valid: BTreeMap::new(), invalid: BTreeMap::new() };
		Self {
			votes,
			own_vote: OwnVoteState::NoVote,
			concluded_invalid: false,
			concluded_valid: false,
			is_confirmed: false,
			is_disputed: false,
		}
	}

	/// Create a new `VoteState` from already existing votes.
	pub fn new<'a>(votes: CandidateVotes, env: &CandidateEnvironment<'a>) -> Self {
		let own_vote = OwnVoteState::new(&votes, env);

		let n_validators = env.validators().len();

		let supermajority_threshold =
			polkadot_primitives::v2::supermajority_threshold(n_validators);

		let concluded_invalid = votes.invalid.len() >= supermajority_threshold;
		let concluded_valid = votes.valid.len() >= supermajority_threshold;

		// We have a dispute, if we have votes on both sides:
		let is_disputed = !votes.invalid.is_empty() && !votes.valid.is_empty();

		let byzantine_threshold = polkadot_primitives::v2::byzantine_threshold(n_validators);
		let is_confirmed = votes.voted_indices().len() > byzantine_threshold && is_disputed;

		Self { votes, own_vote, concluded_invalid, concluded_valid, is_confirmed, is_disputed }
	}

	/// Import fresh statements.
	///
	/// Result will be a new state plus information about things that changed due to the import.
	pub fn import_statements(
		self,
		env: &CandidateEnvironment,
		statements: Vec<(SignedDisputeStatement, ValidatorIndex)>,
	) -> ImportResult {
		let (mut votes, old_state) = self.into_old_state();

		let mut new_invalid_voters = Vec::new();
		let mut imported_invalid_votes = 0;
		let mut imported_valid_votes = 0;

		let expected_candidate_hash = votes.candidate_receipt.hash();

		for (statement, val_index) in statements {
			if env
				.validators()
				.get(val_index.0 as usize)
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

			match statement.statement() {
				DisputeStatement::Valid(valid_kind) => {
					let fresh = insert_into_statements(
						&mut votes.valid,
						*valid_kind,
						val_index,
						statement.into_validator_signature(),
					);

					if fresh {
						imported_valid_votes += 1;
					}
				},
				DisputeStatement::Invalid(invalid_kind) => {
					let fresh = insert_into_statements(
						&mut votes.invalid,
						*invalid_kind,
						val_index,
						statement.into_validator_signature(),
					);

					if fresh {
						new_invalid_voters.push(val_index);
						imported_invalid_votes += 1;
					}
				},
			}
		}

		let new_state = Self::new(votes, env);

		ImportResult {
			old_state,
			new_state,
			imported_invalid_votes,
			imported_valid_votes,
			new_invalid_voters,
		}
	}

	/// Retrieve `CandidateReceipt` in `CandidateVotes`.
	pub fn candidate_receipt(&self) -> &CandidateReceipt {
		&self.votes.candidate_receipt
	}

	/// Extract `CandidateVotes` for handling import of new statements.
	fn into_old_state(self) -> (CandidateVotes, VoteState<()>) {
		let VoteState {
			votes,
			own_vote,
			concluded_invalid,
			concluded_valid,
			is_confirmed,
			is_disputed,
		} = self;
		(
			votes,
			VoteState {
				votes: (),
				own_vote,
				concluded_invalid,
				concluded_valid,
				is_confirmed,
				is_disputed,
			},
		)
	}
}

impl<V> VoteState<V> {
	/// Whether or not we have an ongoing dispute.
	pub fn is_disputed(&self) -> bool {
		self.is_disputed
	}

	/// Whether there is an ongoing confirmed dispute.
	///
	/// This checks whether there is a dispute ongoing and we have more than byzantine threshold
	/// votes.
	pub fn is_confirmed(&self) -> bool {
		self.is_confirmed
	}

	/// This machine already cast some vote in that dispute/for that candidate.
	pub fn has_own_vote(&self) -> bool {
		self.own_vote.voted()
	}

	/// Own approval votes if any:
	pub fn own_approval_votes(&self) -> Option<&Vec<ValidVoteData>> {
		self.own_vote.approval_votes()
	}

	/// Whether or not this dispute has already enough valid votes to conclude.
	pub fn is_concluded_valid(&self) -> bool {
		self.concluded_valid
	}

	/// Whether or not this dispute has already enough invalid votes to conclude.
	pub fn is_concluded_invalid(&self) -> bool {
		self.concluded_invalid
	}

	/// Access to underlying votes.
	pub fn votes(&self) -> &V {
		&self.votes
	}
}

/// An ongoing statement/vote import.
pub struct ImportResult {
	/// The state we had before importing new statements.
	old_state: VoteState<()>,
	/// The new state after importing the new statements.
	new_state: VoteState<CandidateVotes>,
	/// New invalid voters as of this import.
	new_invalid_voters: Vec<ValidatorIndex>,
	/// Number of successfully imported valid votes.
	imported_invalid_votes: u32,
	/// Number of successfully imported invalid votes.
	imported_valid_votes: u32,
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
	pub fn old_state(&self) -> &VoteState<()> {
		&self.old_state
	}

	/// State after import
	pub fn new_state(&self) -> &VoteState<CandidateVotes> {
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

	/// Whether we now have a dispute and did not prior to the import.
	pub fn is_freshly_disputed(&self) -> bool {
		!self.old_state().is_disputed() && self.new_state().is_disputed()
	}

	/// Whether we just surpassed the byzantine threshold.
	pub fn is_freshly_confirmed(&self) -> bool {
		!self.old_state().is_confirmed() && self.new_state().is_confirmed()
	}

	/// Whether or not any dispute just concluded valid due to the import.
	pub fn is_freshly_concluded_valid(&self) -> bool {
		!self.old_state().is_concluded_valid() && self.new_state().is_concluded_valid()
	}

	/// Whether or not any dispute just concluded invalid due to the import.
	pub fn is_freshly_concluded_invalid(&self) -> bool {
		!self.old_state().is_concluded_invalid() && self.new_state().is_concluded_invalid()
	}

	/// Whether or not any dispute just concluded either invalid or valid due to the import.
	pub fn is_freshly_concluded(&self) -> bool {
		self.is_freshly_concluded_invalid() || self.is_freshly_concluded_valid()
	}

	/// Modify this `ImportResult`s, by importing additional approval votes.
	///
	/// Both results and `new_state` will be changed as if those approval votes had been in the
	/// original import.
	pub fn import_approval_votes(
		self,
		env: &CandidateEnvironment,
		approval_votes: HashMap<ValidatorIndex, ValidatorSignature>,
	) -> Self {
		let Self {
			old_state,
			new_state,
			new_invalid_voters,
			mut imported_valid_votes,
			imported_invalid_votes,
		} = self;

		let (mut votes, _) = new_state.into_old_state();

		for (index, sig) in approval_votes.into_iter() {
			debug_assert!(
				{
					let pub_key = &env.session_info().validators[index.0 as usize];
					let candidate_hash = votes.candidate_receipt.hash();
					let session_index = env.session_index();
					DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking)
						.check_signature(pub_key, candidate_hash, session_index, &sig)
						.is_ok()
				},
				"Signature check for imported approval votes failed! This is a serious bug!"
			);
			if insert_into_statements(
				&mut votes.valid,
				ValidDisputeStatementKind::ApprovalChecking,
				index,
				sig,
			) {
				imported_valid_votes += 1;
			}
		}

		let new_state = VoteState::new(votes, env);

		Self {
			old_state,
			new_state,
			new_invalid_voters,
			imported_valid_votes,
			imported_invalid_votes,
		}
	}

	/// All done, give me those votes.
	///
	/// Returns: `None` in case nothing has changed (import was redundant).
	pub fn into_updated_votes(self) -> Option<CandidateVotes> {
		if self.votes_changed() {
			let VoteState { votes, .. } = self.new_state;
			Some(votes)
		} else {
			None
		}
	}
}

/// Find indices controlled by this validator:
/// TODO: Remove pub
pub fn find_controlled_validator_indices(
	keystore: &LocalKeystore,
	validators: &[ValidatorId],
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

// Returns 'true' if no other vote by that validator was already
// present and 'false' otherwise. Same semantics as `HashSet`.
fn insert_into_statements<T>(
	m: &mut BTreeMap<ValidatorIndex, (T, ValidatorSignature)>,
	tag: T,
	val_index: ValidatorIndex,
	val_signature: ValidatorSignature,
) -> bool {
	m.insert(val_index, (tag, val_signature)).is_none()
}
