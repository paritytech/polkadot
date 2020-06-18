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
use polkadot_primitives::parachain::{
	AbridgedCandidateReceipt, CandidateReceipt, SigningContext, ValidatorId, ValidatorIndex,
	ValidatorPair, ValidatorSignature,
};
use polkadot_primitives::Hash;
use runtime_primitives::traits::AppVerify;
use sp_core::crypto::Pair;

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

/// This helper trait ensures that we can encode Statement as CompactStatement,
/// and anything as itself.
pub trait EncodeAs<T> {
	fn encode_as(&self) -> Vec<u8>;
}

impl<T: Encode> EncodeAs<T> for T {
	fn encode_as(&self) -> Vec<u8> {
		self.encode()
	}
}

/// A statement about the validity of a parachain candidate.
///
/// This variant should only be used in the production of `SignedStatement`s. The only difference between
/// this enum and `Statement` is that the `Seconded` variant contains a `Hash` instead of a `CandidateReceipt`.
/// The rationale behind the difference is that a `CandidateReceipt` contains `HeadData`, which does not have
/// bounded size. By using this enum instead, we ensure that the production and validation of signatures is fast
/// while retaining their necessary cryptographic properties.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
enum CompactStatement {
	/// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
	#[codec(index = "1")]
	Seconded(Hash),
	/// A statement about the validity of a candidate, based on candidate's hash.
	#[codec(index = "2")]
	Valid(Hash),
	/// A statement about the invalidity of a candidate.
	#[codec(index = "3")]
	Invalid(Hash),
}

impl EncodeAs<CompactStatement> for Statement {
	fn encode_as(&self) -> Vec<u8> {
		let statement = match *self {
			Statement::Seconded(ref c) => {
				polkadot_primitives::parachain::Statement::Candidate(c.hash())
			}
			Statement::Valid(hash) => polkadot_primitives::parachain::Statement::Valid(hash),
			Statement::Invalid(hash) => polkadot_primitives::parachain::Statement::Invalid(hash),
		};
		statement.encode()
	}
}

/// A signed type which encapsulates the common desire to sign some data and validate a signature.
///
/// Note that the internal fields are not public; they are all accessable by immutable getters.
/// This reduces the chance that they are accidentally mutated, invalidating the signature.
pub struct Signed<Payload, RealPayload = Payload> {
	/// The payload is part of the signed data. The rest is the signing context,
	/// which is known both at signing and at validation.
	payload: Payload,
	/// The index of the validator signing this statement.
	validator_index: ValidatorIndex,
	/// The signature by the validator of the signed payload.
	signature: ValidatorSignature,
	/// This ensures the real payload is tracked at the typesystem level.
	real_payload: std::marker::PhantomData<RealPayload>,
}

// We can't bound this on `Payload: Into<RealPayload>` beacuse that conversion consumes
// the payload, and we don't want that. We can't bound it on `Payload: AsRef<RealPayload>`
// because there's no blanket impl of `AsRef<T> for T`. In the end, we just invent our
// own trait which does what we need: EncodeAs.
impl<Payload: EncodeAs<RealPayload>, RealPayload: Encode> Signed<Payload, RealPayload> {
	fn payload_data(payload: &Payload, context: SigningContext) -> Vec<u8> {
		(payload.encode_as(), context).encode()
	}

	pub fn sign(
		payload: Payload,
		context: SigningContext,
		validator_index: ValidatorIndex,
		key: &ValidatorPair,
	) -> Self {
		let data = Self::payload_data(&payload, context);
		let signature = key.sign(&data);
		Self {
			payload,
			validator_index,
			signature,
			real_payload: std::marker::PhantomData,
		}
	}

	pub fn validate(&self, context: SigningContext, key: &ValidatorId) -> bool {
		let data = Self::payload_data(&self.payload, context);
		self.signature.verify(data.as_slice(), key)
	}

	#[inline]
	pub fn payload(&self) -> &Payload {
		&self.payload
	}

	#[inline]
	pub fn validator_index(&self) -> ValidatorIndex {
		self.validator_index
	}

	#[inline]
	pub fn signature(&self) -> &ValidatorSignature {
		&self.signature
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

/// A misbehaviour report.
pub enum MisbehaviorReport {
	/// These validator nodes disagree on this candidate's validity, please figure it out
	///
	/// Most likely, the list of statments all agree except for the final one. That's not
	/// guaranteed, though; if somehow we become aware of lots of
	/// statements disagreeing about the validity of a candidate before taking action,
	/// this message should be dispatched with all of them, in arbitrary order.
	///
	/// This variant is also used when our own validity checks disagree with others'.
	CandidateValidityDisagreement(CandidateReceipt, Vec<SignedStatement>),
	/// I've noticed a peer contradicting itself about a particular candidate
	SelfContradiction(CandidateReceipt, SignedStatement, SignedStatement),
	/// This peer has seconded more than one parachain candidate for this relay parent head
	DoubleVote(CandidateReceipt, SignedStatement, SignedStatement),
}
