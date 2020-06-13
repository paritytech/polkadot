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

use runtime_primitives::traits::AppVerify;
use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{
	AbridgedCandidateReceipt, SigningContext, ValidatorSignature,
	ValidatorIndex, ValidatorId,
};
use parity_scale_codec::{Encode, Decode};

/// A statement, where the candidate receipt is included in the `Seconded` variant.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Statement {
	/// A statement that a validator seconds a candidate.
	#[codec(index = "1")]
	Seconded(AbridgedCandidateReceipt),
	/// A statement that a validator has deemed a candidate valid.
	#[codec(index = "2")]
	Valid(Hash),
	/// A statement that a validator has deeped a candidate invalid.
	#[codec(index = "3")]
	Invalid(Hash),
}

impl Statement {
	/// Get the signing payload of the statement.
	pub fn signing_payload(&self, context: &SigningContext) -> Vec<u8> {
		// convert to fully hash-based payload.
		let statement = match *self {
			Statement::Seconded(ref c) => polkadot_primitives::parachain::Statement::Candidate(c.hash()),
			Statement::Valid(hash) => polkadot_primitives::parachain::Statement::Valid(hash),
			Statement::Invalid(hash) => polkadot_primitives::parachain::Statement::Invalid(hash),
		};

		statement.signing_payload(context)
	}
}

/// A statement, the corresponding signature, and the index of the sender.
///
/// Signing context and validator set should be apparent from context.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct SignedStatement {
	/// The statement signed.
	pub statement: Statement,
	/// The signature of the validator.
	pub signature: ValidatorSignature,
	/// The index in the validator set of the signing validator. Which validator set should
	/// be apparent from context.
	pub sender: ValidatorIndex,
}

impl SignedStatement {
	/// Check the signature on a statement. Provide a list of validators to index into
	/// and the context in which the statement is presumably signed.
	///
	/// Returns an error if out of bounds or the signature is invalid. Otherwise, returns Ok.
	pub fn check_signature(
		&self,
		validators: &[ValidatorId],
		signing_context: &SigningContext,
	) -> Result<(), ()> {
		let validator = validators.get(self.sender as usize).ok_or(())?;
		let payload = self.statement.signing_payload(signing_context);

		if self.signature.verify(&payload[..], validator) {
			Ok(())
		} else {
			Err(())
		}
	}
}
