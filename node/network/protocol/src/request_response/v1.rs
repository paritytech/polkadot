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

use polkadot_node_primitives::{
	AvailableData, DisputeMessage, ErasureChunk, PoV, Proof, UncheckedDisputeMessage,
};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, CommittedCandidateReceipt, Hash, Id as ParaId, ValidatorIndex,
};

use super::{IsRequest, Protocol};

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
	pub proof: Proof,
}

impl From<ErasureChunk> for ChunkResponse {
	fn from(ErasureChunk { chunk, index: _, proof }: ErasureChunk) -> Self {
		ChunkResponse { chunk, proof }
	}
}

impl ChunkResponse {
	/// Re-build an `ErasureChunk` from response and request.
	pub fn recombine_into_chunk(self, req: &ChunkFetchingRequest) -> ErasureChunk {
		ErasureChunk { chunk: self.chunk, proof: self.proof, index: req.index }
	}
}

impl IsRequest for ChunkFetchingRequest {
	type Response = ChunkFetchingResponse;
	const PROTOCOL: Protocol = Protocol::ChunkFetchingV1;
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
	Collation(CandidateReceipt, PoV),
}

impl IsRequest for CollationFetchingRequest {
	type Response = CollationFetchingResponse;
	const PROTOCOL: Protocol = Protocol::CollationFetchingV1;
}

/// Request the advertised collation at that relay-parent.
#[derive(Debug, Clone, Encode, Decode)]
pub struct PoVFetchingRequest {
	/// Candidate we want a PoV for.
	pub candidate_hash: CandidateHash,
}

/// Responses to `PoVFetchingRequest`.
#[derive(Debug, Clone, Encode, Decode)]
pub enum PoVFetchingResponse {
	/// Deliver requested PoV.
	#[codec(index = 0)]
	PoV(PoV),
	/// PoV was not found in store.
	#[codec(index = 1)]
	NoSuchPoV,
}

impl IsRequest for PoVFetchingRequest {
	type Response = PoVFetchingResponse;
	const PROTOCOL: Protocol = Protocol::PoVFetchingV1;
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
	const PROTOCOL: Protocol = Protocol::AvailableDataFetchingV1;
}

/// Request for fetching a large statement via request/response.
#[derive(Debug, Clone, Encode, Decode)]
pub struct StatementFetchingRequest {
	/// Data needed to locate and identify the needed statement.
	pub relay_parent: Hash,
	/// Hash of candidate that was used create the `CommitedCandidateRecept`.
	pub candidate_hash: CandidateHash,
}

/// Respond with found full statement.
///
/// In this protocol the requester will only request data it was previously notified about,
/// therefore not having the data is not really an option and would just result in a
/// `RequestFailure`.
#[derive(Debug, Clone, Encode, Decode)]
pub enum StatementFetchingResponse {
	/// Data missing to reconstruct the full signed statement.
	#[codec(index = 0)]
	Statement(CommittedCandidateReceipt),
}

impl IsRequest for StatementFetchingRequest {
	type Response = StatementFetchingResponse;
	const PROTOCOL: Protocol = Protocol::StatementFetchingV1;
}

/// A dispute request.
///
/// Contains an invalid vote a valid one for a particular candidate in a given session.
#[derive(Clone, Encode, Decode, Debug)]
pub struct DisputeRequest(pub UncheckedDisputeMessage);

impl From<DisputeMessage> for DisputeRequest {
	fn from(msg: DisputeMessage) -> Self {
		Self(msg.into())
	}
}

/// Possible responses to a `DisputeRequest`.
#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum DisputeResponse {
	/// Recipient successfully processed the dispute request.
	#[codec(index = 0)]
	Confirmed,
}

impl IsRequest for DisputeRequest {
	type Response = DisputeResponse;
	const PROTOCOL: Protocol = Protocol::DisputeSendingV1;
}
