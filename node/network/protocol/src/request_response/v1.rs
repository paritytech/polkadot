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

//! Requests and responses as sent over the wire for the individual protocols.

use parity_scale_codec::{Decode, Encode};

use polkadot_primitives::v1::{CandidateHash, ErasureChunk, ValidatorIndex};

use super::request::IsRequest;
use super::Protocol;

/// Request an availability chunk.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AvailabilityFetchingRequest {
	candidate_hash: CandidateHash,
	index: ValidatorIndex,
}

/// Receive a rqeuested erasure chunk.
#[derive(Debug, Clone, Encode, Decode)]
pub enum AvailabilityFetchingResponse {
	/// The requested chunk.
	#[codec(index = 0)]
	Chunk(ErasureChunk),
}

impl IsRequest for AvailabilityFetchingRequest {
	type Response = AvailabilityFetchingResponse;
	const PROTOCOL: Protocol = Protocol::AvailabilityFetching;
}
