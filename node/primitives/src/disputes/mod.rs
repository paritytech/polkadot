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

use std::collections::{
	btree_map::{Entry as Bentry, Keys as Bkeys},
	BTreeMap, BTreeSet,
};

use parity_scale_codec::{Decode, Encode};

use sp_application_crypto::AppKey;
use sp_keystore::{CryptoStore, Error as KeystoreError, SyncCryptoStorePtr};

use super::{Statement, UncheckedSignedFullStatement};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, DisputeStatement, InvalidDisputeStatementKind, SessionIndex,
	SigningContext, ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorSignature,
};

/// `DisputeMessage` and related types.
mod message;
pub use message::{DisputeMessage, Error as DisputeMessageCheckError, UncheckedDisputeMessage};
mod status;
pub use status::{dispute_is_inactive, DisputeStatus, Timestamp, ACTIVE_DURATION_SECS};

/// A checked dispute statement from an associated validator.
#[derive(Debug, Clone)]
pub struct SignedDisputeStatement {
	dispute_statement: DisputeStatement,
	candidate_hash: CandidateHash,
	validator_public: ValidatorId,
	validator_signature: ValidatorSignature,
	session_index: SessionIndex,
}

/// Tracked votes on candidates, for the purposes of dispute resolution.
#[derive(Debug, Clone)]
pub struct CandidateVotes {
	/// The receipt of the candidate itself.
	pub candidate_receipt: CandidateReceipt,
	/// Votes of validity, sorted by validator index.
	pub valid: ValidCandidateVotes,
	/// Votes of invalidity, sorted by validator index.
	pub invalid: BTreeMap<ValidatorIndex, (InvalidDisputeStatementKind, ValidatorSignature)>,
}

/// Type alias for retrieving valid votes from `CandidateVotes`
pub type ValidVoteData = (ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature));

/// Type alias for retrieving invalid votes from `CandidateVotes`
pub type InvalidVoteData = (ValidatorIndex, (InvalidDisputeStatementKind, ValidatorSignature));

impl CandidateVotes {
	/// Get the set of all validators who have votes in the set, ascending.
	pub fn voted_indices(&self) -> BTreeSet<ValidatorIndex> {
		let mut keys: BTreeSet<_> = self.valid.keys().cloned().collect();
		keys.extend(self.invalid.keys().cloned());
		keys
	}
}

#[derive(Debug, Clone)]
/// Valid candidate votes.
///
/// Prefere backing votes over other votes.
pub struct ValidCandidateVotes {
	votes: BTreeMap<ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature)>,
}

impl ValidCandidateVotes {
	/// Create new empty `ValidCandidateVotes`
	pub fn new() -> Self {
		Self { votes: BTreeMap::new() }
	}
	/// Insert a vote, replacing any already existing vote.
	///
	/// Except, for backing votes: Backing votes are always kept, and will never get overridden.
	/// Import of other king of `valid` votes, will be ignored if a backing vote is already
	/// present. Any already existing `valid` vote, will be overridden by any given backing vote.
	///
	/// Returns: true, if the insert had any effect.
	pub fn insert_vote(
		&mut self,
		validator_index: ValidatorIndex,
		kind: ValidDisputeStatementKind,
		sig: ValidatorSignature,
	) -> bool {
		match self.votes.entry(validator_index) {
			Bentry::Vacant(vacant) => {
				vacant.insert((kind, sig));
				true
			},
			Bentry::Occupied(mut occupied) => match occupied.get().0 {
				ValidDisputeStatementKind::BackingValid(_) |
				ValidDisputeStatementKind::BackingSeconded(_) => false,
				ValidDisputeStatementKind::Explicit |
				ValidDisputeStatementKind::ApprovalChecking => {
					occupied.insert((kind, sig));
					kind != occupied.get().0
				},
			},
		}
	}

	/// Retain any votes that match the given criteria.
	pub fn retain<F>(&mut self, f: F)
	where
		F: FnMut(&ValidatorIndex, &mut (ValidDisputeStatementKind, ValidatorSignature)) -> bool,
	{
		self.votes.retain(f)
	}

	/// Get all the validator indeces we have votes for.
	pub fn keys(
		&self,
	) -> Bkeys<'_, ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature)> {
		self.votes.keys()
	}

	/// Get read only direct access to underlying map.
	pub fn raw(
		&self,
	) -> &BTreeMap<ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature)> {
		&self.votes
	}
}

impl FromIterator<(ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature))>
	for ValidCandidateVotes
{
	fn from_iter<T>(iter: T) -> Self
	where
		T: IntoIterator<Item = (ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature))>,
	{
		Self { votes: BTreeMap::from_iter(iter) }
	}
}

impl From<ValidCandidateVotes>
	for BTreeMap<ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature)>
{
	fn from(wrapped: ValidCandidateVotes) -> Self {
		wrapped.votes
	}
}
impl IntoIterator for ValidCandidateVotes {
	type Item = (ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature));
	type IntoIter = <BTreeMap<ValidatorIndex, (ValidDisputeStatementKind, ValidatorSignature)> as IntoIterator>::IntoIter;

	fn into_iter(self) -> Self::IntoIter {
		self.votes.into_iter()
	}
}

impl SignedDisputeStatement {
	/// Create a new `SignedDisputeStatement` from information
	/// that is available on-chain, and hence already can be trusted.
	///
	/// Attention: Not to be used other than with guaranteed fetches.
	pub fn new_unchecked_from_trusted_source(
		dispute_statement: DisputeStatement,
		candidate_hash: CandidateHash,
		session_index: SessionIndex,
		validator_public: ValidatorId,
		validator_signature: ValidatorSignature,
	) -> Self {
		SignedDisputeStatement {
			dispute_statement,
			candidate_hash,
			validator_public,
			validator_signature,
			session_index,
		}
	}

	/// Create a new `SignedDisputeStatement`, which is only possible by checking the signature.
	pub fn new_checked(
		dispute_statement: DisputeStatement,
		candidate_hash: CandidateHash,
		session_index: SessionIndex,
		validator_public: ValidatorId,
		validator_signature: ValidatorSignature,
	) -> Result<Self, ()> {
		dispute_statement
			.check_signature(&validator_public, candidate_hash, session_index, &validator_signature)
			.map(|_| SignedDisputeStatement {
				dispute_statement,
				candidate_hash,
				validator_public,
				validator_signature,
				session_index,
			})
	}

	/// Sign this statement with the given keystore and key. Pass `valid = true` to
	/// indicate validity of the candidate, and `valid = false` to indicate invalidity.
	pub async fn sign_explicit(
		keystore: &SyncCryptoStorePtr,
		valid: bool,
		candidate_hash: CandidateHash,
		session_index: SessionIndex,
		validator_public: ValidatorId,
	) -> Result<Option<Self>, KeystoreError> {
		let dispute_statement = if valid {
			DisputeStatement::Valid(ValidDisputeStatementKind::Explicit)
		} else {
			DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit)
		};

		let data = dispute_statement.payload_data(candidate_hash, session_index);
		let signature = CryptoStore::sign_with(
			&**keystore,
			ValidatorId::ID,
			&validator_public.clone().into(),
			&data,
		)
		.await?;

		let signature = match signature {
			Some(sig) =>
				sig.try_into().map_err(|_| KeystoreError::KeyNotSupported(ValidatorId::ID))?,
			None => return Ok(None),
		};

		Ok(Some(Self {
			dispute_statement,
			candidate_hash,
			validator_public,
			validator_signature: signature,
			session_index,
		}))
	}

	/// Access the underlying dispute statement
	pub fn statement(&self) -> &DisputeStatement {
		&self.dispute_statement
	}

	/// Access the underlying candidate hash.
	pub fn candidate_hash(&self) -> &CandidateHash {
		&self.candidate_hash
	}

	/// Access the underlying validator public key.
	pub fn validator_public(&self) -> &ValidatorId {
		&self.validator_public
	}

	/// Access the underlying validator signature.
	pub fn validator_signature(&self) -> &ValidatorSignature {
		&self.validator_signature
	}

	/// Consume self to return the signature.
	pub fn into_validator_signature(self) -> ValidatorSignature {
		self.validator_signature
	}

	/// Access the underlying session index.
	pub fn session_index(&self) -> SessionIndex {
		self.session_index
	}

	/// Convert a [`SignedFullStatement`] to a [`SignedDisputeStatement`]
	///
	/// As [`SignedFullStatement`] contains only the validator index and
	/// not the validator public key, the public key must be passed as well,
	/// along with the signing context.
	///
	/// This does signature checks again with the data provided.
	pub fn from_backing_statement(
		backing_statement: &UncheckedSignedFullStatement,
		signing_context: SigningContext,
		validator_public: ValidatorId,
	) -> Result<Self, ()> {
		let (statement_kind, candidate_hash) = match backing_statement.unchecked_payload() {
			Statement::Seconded(candidate) => (
				ValidDisputeStatementKind::BackingSeconded(signing_context.parent_hash),
				candidate.hash(),
			),
			Statement::Valid(candidate_hash) => (
				ValidDisputeStatementKind::BackingValid(signing_context.parent_hash),
				*candidate_hash,
			),
		};

		let dispute_statement = DisputeStatement::Valid(statement_kind);
		Self::new_checked(
			dispute_statement,
			candidate_hash,
			signing_context.session_index,
			validator_public,
			backing_statement.unchecked_signature().clone(),
		)
	}
}

/// Any invalid vote (currently only explicit).
#[derive(Clone, Encode, Decode, Debug)]
pub struct InvalidDisputeVote {
	/// The voting validator index.
	pub validator_index: ValidatorIndex,

	/// The validator signature, that can be verified when constructing a
	/// `SignedDisputeStatement`.
	pub signature: ValidatorSignature,

	/// Kind of dispute statement.
	pub kind: InvalidDisputeStatementKind,
}

/// Any valid vote (backing, approval, explicit).
#[derive(Clone, Encode, Decode, Debug)]
pub struct ValidDisputeVote {
	/// The voting validator index.
	pub validator_index: ValidatorIndex,

	/// The validator signature, that can be verified when constructing a
	/// `SignedDisputeStatement`.
	pub signature: ValidatorSignature,

	/// Kind of dispute statement.
	pub kind: ValidDisputeStatementKind,
}
