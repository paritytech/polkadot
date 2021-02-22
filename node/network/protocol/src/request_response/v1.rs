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
#[derive(Debug, Copy, Clone, Encode, Decode)]
pub struct AvailabilityFetchingRequest {
	/// Hash of candidate we want a chunk for.
	pub candidate_hash: CandidateHash,
	/// The index of the chunk to fetch.
	pub index: ValidatorIndex,
}

/// Receive a rqeuested erasure chunk.
#[derive(Debug, Clone, Encode, Decode)]
pub enum AvailabilityFetchingResponse {
	/// The requested chunk data.
	#[codec(index = 0)]
	Chunk(ChunkResponse),
	/// Node was not in possession of the requested chunk.
	#[codec(index = 1)]
	NoSuchChunk,
}

/// Skimmed down variant of `ErasureChunk`.
///
/// Instead of transmitting a full `ErasureChunk` we transmit `ChunkResponse` in
/// `AvailabilityFetchingResponse`, which omits the chunk's index. The index is already known by
/// the requester and by not transmitting it, we ensure the requester is going to use his index
/// value for validating the response, thus making sure he got what he requested.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ChunkResponse {
	/// The erasure-encoded chunk of data belonging to the candidate block.
	pub chunk: Vec<u8>,
	/// Proof for this chunk's branch in the Merkle tree.
	pub proof: Vec<Vec<u8>>,
}

impl From<ErasureChunk> for ChunkResponse {
	fn from(ErasureChunk {chunk, index: _, proof}: ErasureChunk) -> Self {
		ChunkResponse {chunk, proof}
	}
}

impl ChunkResponse {
	/// Re-build an `ErasureChunk` from response and request.
	pub fn recombine_into_chunk(self, req: &AvailabilityFetchingRequest) -> ErasureChunk {
		ErasureChunk {
			chunk: self.chunk,
			proof: self.proof,
			index: req.index,
		}
	}
}

impl IsRequest for AvailabilityFetchingRequest {
	type Response = AvailabilityFetchingResponse;
	const PROTOCOL: Protocol = Protocol::AvailabilityFetching;
}
