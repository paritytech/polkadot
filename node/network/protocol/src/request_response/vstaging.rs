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

//! Requests and responses as sent over the wire for the individual protocols.

use parity_scale_codec::{Decode, Encode};

use polkadot_primitives::vstaging::{
	CandidateHash, CommittedCandidateReceipt, Hash, Id as ParaId, PersistedValidationData,
	UncheckedSignedStatement,
};

use super::{IsRequest, Protocol};
use crate::vstaging::StatementFilter;

/// Request a candidate with statements.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AttestedCandidateRequest {
	/// Hash of the candidate we want to request.
	pub candidate_hash: CandidateHash,
	/// Statement filter with 'OR' semantics, indicating which validators
	/// not to send statements for.
	///
	/// The filter must have exactly the minimum size required to
	/// fit all validators from the backing group.
	///
	/// The response may not contain any statements masked out by this mask.
	pub mask: StatementFilter,
}

/// Response to an `AttestedCandidateRequest`.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AttestedCandidateResponse {
	/// The candidate receipt, with commitments.
	pub candidate_receipt: CommittedCandidateReceipt,
	/// The [`PersistedValidationData`] corresponding to the candidate.
	pub persisted_validation_data: PersistedValidationData,
	/// All known statements about the candidate, in compact form,
	/// omitting `Seconded` statements which were intended to be masked
	/// out.
	pub statements: Vec<UncheckedSignedStatement>,
}

impl IsRequest for AttestedCandidateRequest {
	type Response = AttestedCandidateResponse;
	const PROTOCOL: Protocol = Protocol::AttestedCandidateVStaging;
}

/// Responses as sent by collators.
pub type CollationFetchingResponse = super::v1::CollationFetchingResponse;

/// Request the advertised collation at that relay-parent.
#[derive(Debug, Clone, Encode, Decode)]
pub struct CollationFetchingRequest {
	/// Relay parent collation is built on top of.
	pub relay_parent: Hash,
	/// The `ParaId` of the collation.
	pub para_id: ParaId,
	/// Candidate hash.
	pub candidate_hash: CandidateHash,
}

impl IsRequest for CollationFetchingRequest {
	// The response is the same as for V1.
	type Response = CollationFetchingResponse;
	const PROTOCOL: Protocol = Protocol::CollationFetchingVStaging;
}
