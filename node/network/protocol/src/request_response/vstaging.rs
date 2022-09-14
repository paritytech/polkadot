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
	CandidateHash, CommittedCandidateReceipt, UncheckedSignedStatement,
};

use super::{IsRequest, Protocol};

/// Request a backed candidate packet.
#[derive(Debug, Copy, Clone, Encode, Decode)]
pub struct BackedCandidatePacketRequest {
	/// Hash of the candidate we want to request.
	pub candidate_hash: CandidateHash,
}

/// Response to a backed candidate packet request.
#[derive(Debug, Clone, Encode, Decode)]
pub struct BackedCandidatePacketResponse {
	/// The candidate receipt, with commitments.
	pub candidate_receipt: CommittedCandidateReceipt,
	/// All known statements about the candidate, in compact form.
	pub statements: Vec<UncheckedSignedStatement>,
}

impl IsRequest for BackedCandidatePacketRequest {
	type Response = BackedCandidatePacketResponse;
	const PROTOCOL: Protocol = Protocol::BackedCandidatePacketV2;
}
