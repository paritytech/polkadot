// Copyright (C) Parity Technologies (UK) Ltd.
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

//! `DisputeMessage` and associated types.
//!
//! A `DisputeMessage` is a message that indicates a node participating in a dispute and is used
//! for interfacing with `DisputeDistribution` to send out our vote in a spam detectable way.

use thiserror::Error;

use parity_scale_codec::{Decode, Encode};

use super::{InvalidDisputeVote, SignedDisputeStatement, ValidDisputeVote};
use polkadot_primitives::{
	CandidateReceipt, DisputeStatement, SessionIndex, SessionInfo, ValidatorIndex,
};

/// A dispute initiating/participating message that have been built from signed
/// statements.
///
/// And most likely has been constructed correctly. This is used with
/// `DisputeDistributionMessage::SendDispute` for sending out votes.
///
/// NOTE: This is sent over the wire, any changes are a change in protocol and need to be
/// versioned.
#[derive(Debug, Clone)]
pub struct DisputeMessage(UncheckedDisputeMessage);

/// A `DisputeMessage` where signatures of statements have not yet been checked.
#[derive(Clone, Encode, Decode, Debug)]
pub struct UncheckedDisputeMessage {
	/// The candidate being disputed.
	pub candidate_receipt: CandidateReceipt,

	/// The session the candidate appears in.
	pub session_index: SessionIndex,

	/// The invalid vote data that makes up this dispute.
	pub invalid_vote: InvalidDisputeVote,

	/// The valid vote that makes this dispute request valid.
	pub valid_vote: ValidDisputeVote,
}

/// Things that can go wrong when constructing a `DisputeMessage`.
#[derive(Error, Debug)]
pub enum Error {
	/// The statements concerned different candidates.
	#[error("Candidate hashes of the two votes did not match up")]
	CandidateHashMismatch,

	/// The statements concerned different sessions.
	#[error("Session indices of the two votes did not match up")]
	SessionIndexMismatch,

	/// The valid statement validator key did not correspond to passed in `SessionInfo`.
	#[error("Valid statement validator key did not match session information")]
	InvalidValidKey,

	/// The invalid statement validator key did not correspond to passed in `SessionInfo`.
	#[error("Invalid statement validator key did not match session information")]
	InvalidInvalidKey,

	/// Provided receipt had different hash than the `CandidateHash` in the signed statements.
	#[error("Hash of candidate receipt did not match provided hash")]
	InvalidCandidateReceipt,

	/// Valid statement should have `ValidDisputeStatementKind`.
	#[error("Valid statement has kind `invalid`")]
	ValidStatementHasInvalidKind,

	/// Invalid statement should have `InvalidDisputeStatementKind`.
	#[error("Invalid statement has kind `valid`")]
	InvalidStatementHasValidKind,

	/// Provided index could not be found in `SessionInfo`.
	#[error("The valid statement had an invalid validator index")]
	ValidStatementInvalidValidatorIndex,

	/// Provided index could not be found in `SessionInfo`.
	#[error("The invalid statement had an invalid validator index")]
	InvalidStatementInvalidValidatorIndex,
}

impl DisputeMessage {
	/// Build a `SignedDisputeMessage` and check what can be checked.
	///
	/// This function checks that:
	///
	/// - both statements concern the same candidate
	/// - both statements concern the same session
	/// - the invalid statement is indeed an invalid one
	/// - the valid statement is indeed a valid one
	/// - The passed `CandidateReceipt` has the correct hash (as signed in the statements).
	/// - the given validator indices match with the given `ValidatorId`s in the statements, given a
	///   `SessionInfo`.
	///
	/// We don't check whether the given `SessionInfo` matches the `SessionIndex` in the
	/// statements, because we can't without doing a runtime query. Nevertheless this smart
	/// constructor gives relative strong guarantees that the resulting `SignedDisputeStatement` is
	/// valid and good.  Even the passed `SessionInfo` is most likely right if this function
	/// returns `Some`, because otherwise the passed `ValidatorId`s in the `SessionInfo` at
	/// their given index would very likely not match the `ValidatorId`s in the statements.
	///
	/// So in summary, this smart constructor should be smart enough to prevent from almost all
	/// programming errors that one could realistically make here.
	pub fn from_signed_statements(
		valid_statement: SignedDisputeStatement,
		valid_index: ValidatorIndex,
		invalid_statement: SignedDisputeStatement,
		invalid_index: ValidatorIndex,
		candidate_receipt: CandidateReceipt,
		session_info: &SessionInfo,
	) -> Result<Self, Error> {
		let candidate_hash = *valid_statement.candidate_hash();
		// Check statements concern same candidate:
		if candidate_hash != *invalid_statement.candidate_hash() {
			return Err(Error::CandidateHashMismatch)
		}

		let session_index = valid_statement.session_index();
		if session_index != invalid_statement.session_index() {
			return Err(Error::SessionIndexMismatch)
		}

		let valid_id = session_info
			.validators
			.get(valid_index)
			.ok_or(Error::ValidStatementInvalidValidatorIndex)?;
		let invalid_id = session_info
			.validators
			.get(invalid_index)
			.ok_or(Error::InvalidStatementInvalidValidatorIndex)?;

		if valid_id != valid_statement.validator_public() {
			return Err(Error::InvalidValidKey)
		}

		if invalid_id != invalid_statement.validator_public() {
			return Err(Error::InvalidInvalidKey)
		}

		if candidate_receipt.hash() != candidate_hash {
			return Err(Error::InvalidCandidateReceipt)
		}

		let valid_kind = match valid_statement.statement() {
			DisputeStatement::Valid(v) => v,
			_ => return Err(Error::ValidStatementHasInvalidKind),
		};

		let invalid_kind = match invalid_statement.statement() {
			DisputeStatement::Invalid(v) => v,
			_ => return Err(Error::InvalidStatementHasValidKind),
		};

		let valid_vote = ValidDisputeVote {
			validator_index: valid_index,
			signature: valid_statement.validator_signature().clone(),
			kind: valid_kind.clone(),
		};

		let invalid_vote = InvalidDisputeVote {
			validator_index: invalid_index,
			signature: invalid_statement.validator_signature().clone(),
			kind: *invalid_kind,
		};

		Ok(DisputeMessage(UncheckedDisputeMessage {
			candidate_receipt,
			session_index,
			valid_vote,
			invalid_vote,
		}))
	}

	/// Read only access to the candidate receipt.
	pub fn candidate_receipt(&self) -> &CandidateReceipt {
		&self.0.candidate_receipt
	}

	/// Read only access to the `SessionIndex`.
	pub fn session_index(&self) -> SessionIndex {
		self.0.session_index
	}

	/// Read only access to the invalid vote.
	pub fn invalid_vote(&self) -> &InvalidDisputeVote {
		&self.0.invalid_vote
	}

	/// Read only access to the valid vote.
	pub fn valid_vote(&self) -> &ValidDisputeVote {
		&self.0.valid_vote
	}
}

impl UncheckedDisputeMessage {
	/// Try to recover the two signed dispute votes from an `UncheckedDisputeMessage`.
	pub fn try_into_signed_votes(
		self,
		session_info: &SessionInfo,
	) -> Result<
		(
			CandidateReceipt,
			(SignedDisputeStatement, ValidatorIndex),
			(SignedDisputeStatement, ValidatorIndex),
		),
		(),
	> {
		let Self { candidate_receipt, session_index, valid_vote, invalid_vote } = self;
		let candidate_hash = candidate_receipt.hash();

		let vote_valid = {
			let ValidDisputeVote { validator_index, signature, kind } = valid_vote;
			let validator_public = session_info.validators.get(validator_index).ok_or(())?.clone();

			(
				SignedDisputeStatement::new_checked(
					DisputeStatement::Valid(kind),
					candidate_hash,
					session_index,
					validator_public,
					signature,
				)?,
				validator_index,
			)
		};

		let vote_invalid = {
			let InvalidDisputeVote { validator_index, signature, kind } = invalid_vote;
			let validator_public = session_info.validators.get(validator_index).ok_or(())?.clone();

			(
				SignedDisputeStatement::new_checked(
					DisputeStatement::Invalid(kind),
					candidate_hash,
					session_index,
					validator_public,
					signature,
				)?,
				validator_index,
			)
		};

		Ok((candidate_receipt, vote_valid, vote_invalid))
	}
}

impl From<DisputeMessage> for UncheckedDisputeMessage {
	fn from(message: DisputeMessage) -> Self {
		message.0
	}
}
