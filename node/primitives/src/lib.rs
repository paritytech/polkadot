// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Primitive types used on the node-side.
//!
//! Unlike the `polkadot-primitives` crate, these primitives are only used on the node-side,
//! not shared between the node and the runtime. This crate builds on top of the primitives defined
//! there.

#![deny(missing_docs)]

use futures::Future;
use parity_scale_codec::{Decode, Encode};
use polkadot_primitives::v1::{
	Hash, CommittedCandidateReceipt, CandidateReceipt, CompactStatement,
	EncodeAs, Signed, SigningContext, ValidatorIndex, ValidatorId,
	UpwardMessage, ValidationCode, PersistedValidationData, ValidationData,
	HeadData, PoV, CollatorPair, Id as ParaId, OutboundHrmpMessage, ValidationOutputs, CandidateHash,
};
use polkadot_statement_table::{
	generic::{
		ValidityDoubleVote as TableValidityDoubleVote,
		MultipleCandidates as TableMultipleCandidates,
	},
	v1::Misbehavior as TableMisbehavior,
};
use std::pin::Pin;

pub use sp_core::traits::SpawnNamed;

/// A statement, where the candidate receipt is included in the `Seconded` variant.
///
/// This is the committed candidate receipt instead of the bare candidate receipt. As such,
/// it gives access to the commitments to validators who have not executed the candidate. This
/// is necessary to allow a block-producing validator to include candidates from outside of the para
/// it is assigned to.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum Statement {
	/// A statement that a validator seconds a candidate.
	#[codec(index = "1")]
	Seconded(CommittedCandidateReceipt),
	/// A statement that a validator has deemed a candidate valid.
	#[codec(index = "2")]
	Valid(CandidateHash),
	/// A statement that a validator has deemed a candidate invalid.
	#[codec(index = "3")]
	Invalid(CandidateHash),
}

impl Statement {
	/// Transform this statement into its compact version, which references only the hash
	/// of the candidate.
	pub fn to_compact(&self) -> CompactStatement {
		match *self {
			Statement::Seconded(ref c) => CompactStatement::Candidate(c.hash()),
			Statement::Valid(hash) => CompactStatement::Valid(hash),
			Statement::Invalid(hash) => CompactStatement::Invalid(hash),
		}
	}
}

impl EncodeAs<CompactStatement> for Statement {
	fn encode_as(&self) -> Vec<u8> {
		self.to_compact().encode()
	}
}

/// A statement, the corresponding signature, and the index of the sender.
///
/// Signing context and validator set should be apparent from context.
///
/// This statement is "full" in the sense that the `Seconded` variant includes the candidate receipt.
/// Only the compact `SignedStatement` is suitable for submission to the chain.
pub type SignedFullStatement = Signed<Statement, CompactStatement>;

/// A misbehaviour report.
#[derive(Debug, Clone)]
pub enum MisbehaviorReport {
	/// These validator nodes disagree on this candidate's validity, please figure it out
	///
	/// Most likely, the list of statments all agree except for the final one. That's not
	/// guaranteed, though; if somehow we become aware of lots of
	/// statements disagreeing about the validity of a candidate before taking action,
	/// this message should be dispatched with all of them, in arbitrary order.
	///
	/// This variant is also used when our own validity checks disagree with others'.
	CandidateValidityDisagreement(CandidateReceipt, Vec<SignedFullStatement>),
	/// I've noticed a peer contradicting itself about a particular candidate
	SelfContradiction(CandidateReceipt, SignedFullStatement, SignedFullStatement),
	/// This peer has seconded more than one parachain candidate for this relay parent head
	DoubleVote(SignedFullStatement, SignedFullStatement),
}

/// A utility struct used to convert `TableMisbehavior` to `MisbehaviorReport`s.
pub struct FromTableMisbehavior {
	/// Index of the validator.
	pub id: ValidatorIndex,
	/// The misbehavior reported by the table.
	pub report: TableMisbehavior,
	/// Signing context.
	pub signing_context: SigningContext,
	/// Misbehaving validator's public key.
	pub key: ValidatorId,
}

/// Candidate invalidity details
#[derive(Debug)]
pub enum InvalidCandidate {
	/// Failed to execute.`validate_block`. This includes function panicking.
	ExecutionError(String),
	/// Validation outputs check doesn't pass.
	InvalidOutputs,
	/// Execution timeout.
	Timeout,
	/// Validation input is over the limit.
	ParamsTooLarge(u64),
	/// Code size is over the limit.
	CodeTooLarge(u64),
	/// Validation function returned invalid data.
	BadReturn,
	/// Invalid relay chain parent.
	BadParent,
	/// POV hash does not match.
	HashMismatch,
	/// Bad collator signature.
	BadSignature,
}

/// Result of the validation of the candidate.
#[derive(Debug)]
pub enum ValidationResult {
	/// Candidate is valid. The validation process yields these outputs and the persisted validation
	/// data used to form inputs.
	Valid(ValidationOutputs, PersistedValidationData),
	/// Candidate is invalid.
	Invalid(InvalidCandidate),
}

impl std::convert::TryFrom<FromTableMisbehavior> for MisbehaviorReport {
	type Error = ();

	fn try_from(f: FromTableMisbehavior) -> Result<Self, Self::Error> {
		match f.report {
			TableMisbehavior::ValidityDoubleVote(
				TableValidityDoubleVote::IssuedAndValidity((c, s1), (d, s2))
			) => {
				let receipt = c.clone();
				let signed_1 = SignedFullStatement::new(
					Statement::Seconded(c),
					f.id,
					s1,
					&f.signing_context,
					&f.key,
				).ok_or(())?;
				let signed_2 = SignedFullStatement::new(
					Statement::Valid(d),
					f.id,
					s2,
					&f.signing_context,
					&f.key,
				).ok_or(())?;

				Ok(MisbehaviorReport::SelfContradiction(receipt.to_plain(), signed_1, signed_2))
			}
			TableMisbehavior::ValidityDoubleVote(
				TableValidityDoubleVote::IssuedAndInvalidity((c, s1), (d, s2))
			) => {
				let receipt = c.clone();
				let signed_1 = SignedFullStatement::new(
					Statement::Seconded(c),
					f.id,
					s1,
					&f.signing_context,
					&f.key,
				).ok_or(())?;
				let signed_2 = SignedFullStatement::new(
					Statement::Invalid(d),
					f.id,
					s2,
					&f.signing_context,
					&f.key,
				).ok_or(())?;

				Ok(MisbehaviorReport::SelfContradiction(receipt.to_plain(), signed_1, signed_2))
			}
			TableMisbehavior::ValidityDoubleVote(
				TableValidityDoubleVote::ValidityAndInvalidity(c, s1, s2)
			) => {
				let signed_1 = SignedFullStatement::new(
					Statement::Valid(c.hash()),
					f.id,
					s1,
					&f.signing_context,
					&f.key,
				).ok_or(())?;
				let signed_2 = SignedFullStatement::new(
					Statement::Invalid(c.hash()),
					f.id,
					s2,
					&f.signing_context,
					&f.key,
				).ok_or(())?;

				Ok(MisbehaviorReport::SelfContradiction(c.to_plain(), signed_1, signed_2))
			}
			TableMisbehavior::MultipleCandidates(
				TableMultipleCandidates {
					first,
					second,
				}
			) => {
				let signed_1 = SignedFullStatement::new(
					Statement::Seconded(first.0),
					f.id,
					first.1,
					&f.signing_context,
					&f.key,
				).ok_or(())?;

				let signed_2 = SignedFullStatement::new(
					Statement::Seconded(second.0),
					f.id,
					second.1,
					&f.signing_context,
					&f.key,
				).ok_or(())?;

				Ok(MisbehaviorReport::DoubleVote(signed_1, signed_2))
			}
			_ => Err(()),
		}
	}
}

/// The output of a collator.
///
/// This differs from `CandidateCommitments` in two ways:
///
/// - does not contain the erasure root; that's computed at the Polkadot level, not at Cumulus
/// - contains a proof of validity.
#[derive(Clone, Encode, Decode)]
pub struct Collation<BlockNumber = polkadot_primitives::v1::BlockNumber> {
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// The horizontal messages sent by the parachain.
	pub horizontal_messages: Vec<OutboundHrmpMessage<ParaId>>,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	pub head_data: HeadData,
	/// Proof to verify the state transition of the parachain.
	pub proof_of_validity: PoV,
	/// The number of messages processed from the DMQ.
	pub processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are processed.
	pub hrmp_watermark: BlockNumber,
}

/// Configuration for the collation generator
pub struct CollationGenerationConfig {
	/// Collator's authentication key, so it can sign things.
	pub key: CollatorPair,
	/// Collation function.
	///
	/// Will be called with the hash of the relay chain block the parachain
	/// block should be build on and the [`ValidationData`] that provides
	/// information about the state of the parachain on the relay chain.
	pub collator: Box<dyn Fn(Hash, &ValidationData) -> Pin<Box<dyn Future<Output = Option<Collation>> + Send>> + Send + Sync>,
	/// The parachain that this collator collates for
	pub para_id: ParaId,
}

impl std::fmt::Debug for CollationGenerationConfig {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "CollationGenerationConfig {{ ... }}")
	}
}
