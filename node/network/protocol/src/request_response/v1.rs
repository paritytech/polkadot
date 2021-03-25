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

use polkadot_primitives::v1::{
	AvailableData, CandidateHash, CandidateReceipt, ErasureChunk, ValidatorIndex,
	CompressedPoV, Hash,
};
use polkadot_primitives::v1::Id as ParaId;

use super::request::IsRequest;
use super::Protocol;

/// Request an availability chunk.
#[derive(Debug, Copy, Clone, Encode, Decode)]
pub struct ChunkFetchingRequest {
	/// Hash of candidate we want a chunk for.
	pub candidate_hash: CandidateHash,
	/// The index of the chunk to fetch.
	pub index: ValidatorIndex,
}

/// Receive a requested erasure chunk.
#[derive(Debug, Clone, Encode, Decode)]
pub enum ChunkFetchingResponse {
	/// The requested chunk data.
	#[codec(index = 0)]
	Chunk(ChunkResponse),
	/// Node was not in possession of the requested chunk.
	#[codec(index = 1)]
	NoSuchChunk,
}

impl From<Option<ChunkResponse>> for ChunkFetchingResponse {
	fn from(x: Option<ChunkResponse>) -> Self {
		match x {
			Some(c) => ChunkFetchingResponse::Chunk(c),
			None => ChunkFetchingResponse::NoSuchChunk,
		}
	}
}

/// Skimmed down variant of `ErasureChunk`.
///
/// Instead of transmitting a full `ErasureChunk` we transmit `ChunkResponse` in
/// `ChunkFetchingResponse`, which omits the chunk's index. The index is already known by
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
	pub fn recombine_into_chunk(self, req: &ChunkFetchingRequest) -> ErasureChunk {
		ErasureChunk {
			chunk: self.chunk,
			proof: self.proof,
			index: req.index,
		}
	}
}

impl IsRequest for ChunkFetchingRequest {
	type Response = ChunkFetchingResponse;
	const PROTOCOL: Protocol = Protocol::ChunkFetching;
}

/// Request the advertised collation at that relay-parent.
#[derive(Debug, Clone, Encode, Decode)]
pub struct CollationFetchingRequest {
	/// Relay parent we want a collation for.
	pub relay_parent: Hash,
	/// The `ParaId` of the collation.
	pub para_id: ParaId,
}

/// Responses as sent by collators.
#[derive(Debug, Clone, Encode, Decode)]
pub enum CollationFetchingResponse {
	/// Deliver requested collation.
	#[codec(index = 0)]
	Collation(CandidateReceipt, CompressedPoV),
}

impl IsRequest for CollationFetchingRequest {
	type Response = CollationFetchingResponse;
	const PROTOCOL: Protocol = Protocol::CollationFetching;
}

/// Request the entire available data for a candidate.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AvailableDataFetchingRequest {
	/// The candidate hash to get the available data for.
	pub candidate_hash: CandidateHash,
}

/// Receive a requested available data.
#[derive(Debug, Clone, Encode, Decode)]
pub enum AvailableDataFetchingResponse {
	/// The requested data.
	#[codec(index = 0)]
	AvailableData(AvailableData),
	/// Node was not in possession of the requested data.
	#[codec(index = 1)]
	NoSuchData,
}

impl From<Option<AvailableData>> for AvailableDataFetchingResponse {
	fn from(x: Option<AvailableData>) -> Self {
		match x {
			Some(data) => AvailableDataFetchingResponse::AvailableData(data),
			None => AvailableDataFetchingResponse::NoSuchData,
		}
	}
}

impl IsRequest for AvailableDataFetchingRequest {
	type Response = AvailableDataFetchingResponse;
	const PROTOCOL: Protocol = Protocol::AvailableDataFetching;
}
