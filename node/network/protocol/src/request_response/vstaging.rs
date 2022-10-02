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

use bitvec::{order::Lsb0, vec::BitVec};
use polkadot_primitives::vstaging::{
	CandidateHash, CommittedCandidateReceipt, PersistedValidationData, UncheckedSignedStatement,
};

use super::{IsRequest, Protocol};

/// Request a candidate with statements.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AttestedCandidateRequest {
	/// Hash of the candidate we want to request.
	pub candidate_hash: CandidateHash,
	/// bitfield with 'AND' semantics, indicating which validators
	/// to send `Seconded` statements for.
	///
	/// The mask must have exactly the minimum size required to
	/// fit all validators from the backing group.
	///
	/// The response may not contain any `Seconded` statements outside
	/// of this mask.
	pub seconded_mask: BitVec<u8, Lsb0>,
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
	const PROTOCOL: Protocol = Protocol::AttestedCandidateV2;
}
