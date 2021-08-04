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

use std::convert::TryInto;

use parity_scale_codec::{Decode, Encode};

use sp_application_crypto::AppKey;
use sp_keystore::{CryptoStore, Error as KeystoreError, SyncCryptoStorePtr};

use super::{Statement, UncheckedSignedFullStatement};
use polkadot_primitives::v1::{
	CandidateHash, CandidateReceipt, DisputeStatement, InvalidDisputeStatementKind, SessionIndex,
	SigningContext, ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorSignature,
};

/// `DisputeMessage` and related types.
mod message;
pub use message::{DisputeMessage, Error as DisputeMessageCheckError, UncheckedDisputeMessage};

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
	pub valid: Vec<(ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
	/// Votes of invalidity, sorted by validator index.
	pub invalid: Vec<(InvalidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
}

impl CandidateVotes {
	/// Get the set of all validators who have votes in the set, ascending.
	pub fn voted_indices(&self) -> Vec<ValidatorIndex> {
		let mut v: Vec<_> =
			self.valid.iter().map(|x| x.1).chain(self.invalid.iter().map(|x| x.1)).collect();

		v.sort();
		v.dedup();

		v
	}
}

impl SignedDisputeStatement {
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
