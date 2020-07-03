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

use parity_scale_codec::{Decode, Encode};
use polkadot_primitives::{Hash,
	parachain::{
		AbridgedCandidateReceipt, CompactStatement,
		EncodeAs, Signed, SigningContext, ValidatorIndex, ValidatorId,
	}
};
use polkadot_statement_table::{
	generic::ValidityDoubleVote as TableValidityDoubleVote,
	Misbehavior as TableMisbehavior,
};

/// A statement, where the candidate receipt is included in the `Seconded` variant.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Statement {
	/// A statement that a validator seconds a candidate.
	#[codec(index = "1")]
	Seconded(AbridgedCandidateReceipt),
	/// A statement that a validator has deemed a candidate valid.
	#[codec(index = "2")]
	Valid(Hash),
	/// A statement that a validator has deemed a candidate invalid.
	#[codec(index = "3")]
	Invalid(Hash),
}

impl EncodeAs<CompactStatement> for Statement {
	fn encode_as(&self) -> Vec<u8> {
		let statement = match *self {
			Statement::Seconded(ref c) => {
				polkadot_primitives::parachain::CompactStatement::Candidate(c.hash())
			}
			Statement::Valid(hash) => polkadot_primitives::parachain::CompactStatement::Valid(hash),
			Statement::Invalid(hash) => polkadot_primitives::parachain::CompactStatement::Invalid(hash),
		};
		statement.encode()
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
#[derive(Debug)]
pub enum MisbehaviorReport {
	/// These validator nodes disagree on this candidate's validity, please figure it out
	///
	/// Most likely, the list of statments all agree except for the final one. That's not
	/// guaranteed, though; if somehow we become aware of lots of
	/// statements disagreeing about the validity of a candidate before taking action,
	/// this message should be dispatched with all of them, in arbitrary order.
	///
	/// This variant is also used when our own validity checks disagree with others'.
	CandidateValidityDisagreement(AbridgedCandidateReceipt, Vec<SignedFullStatement>),
	/// I've noticed a peer contradicting itself about a particular candidate
	SelfContradiction(AbridgedCandidateReceipt, SignedFullStatement, SignedFullStatement),
	/// This peer has seconded more than one parachain candidate for this relay parent head
	DoubleVote(AbridgedCandidateReceipt, SignedFullStatement, SignedFullStatement),
}

pub struct FromTableMisbehavior {
	pub id: ValidatorIndex,

	pub report: TableMisbehavior,

	pub signing_context: SigningContext,

	pub key: ValidatorId,
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

				Ok(MisbehaviorReport::DoubleVote(receipt, signed_1, signed_2))
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

				Ok(MisbehaviorReport::DoubleVote(receipt, signed_1, signed_2))
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

				Ok(MisbehaviorReport::SelfContradiction(c, signed_1, signed_2))
			}
			_ => {
				// TODO: match other cases
				Err(())
			}
		}
	}
}

/// A unique identifier for a network protocol.
pub type ProtocolId = [u8; 4];

/// A succinct representation of a peer's view. This consists of a bounded amount of chain heads.
///
/// Up to `N` (5?) chain heads.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct View(pub Vec<Hash>);
