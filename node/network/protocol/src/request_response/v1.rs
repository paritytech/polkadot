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
pub struct ChunkFetchingV1Request {
	/// Hash of candidate we want a chunk for.
	pub candidate_hash: CandidateHash,
	/// The index of the chunk to fetch.
	pub index: ValidatorIndex,
}

/// Receive a requested erasure chunk.
#[derive(Debug, Clone, Encode, Decode)]
pub enum ChunkFetchingV1Response {
	/// The requested chunk data.
	#[codec(index = 0)]
	Chunk(ChunkResponse),
	/// Node was not in possession of the requested chunk.
	#[codec(index = 1)]
	NoSuchChunk,
}

impl From<Option<ChunkResponse>> for ChunkFetchingV1Response {
	fn from(x: Option<ChunkResponse>) -> Self {
		match x {
			Some(c) => ChunkFetchingV1Response::Chunk(c),
			None => ChunkFetchingV1Response::NoSuchChunk,
		}
	}
}

/// Skimmed down variant of `ErasureChunk`.
///
/// Instead of transmitting a full `ErasureChunk` we transmit `ChunkResponse` in
/// `ChunkFetchingV1Response`, which omits the chunk's index. The index is already known by
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
	pub fn recombine_into_chunk(self, req: &ChunkFetchingV1Request) -> ErasureChunk {
		ErasureChunk { chunk: self.chunk, proof: self.proof, index: req.index }
	}
}

impl IsRequest for ChunkFetchingV1Request {
	type Response = ChunkFetchingV1Response;
	const PROTOCOL: Protocol = Protocol::ChunkFetchingV1;
}

/// Request the advertised collation at that relay-parent.
#[derive(Debug, Clone, Encode, Decode)]
pub struct CollationFetchingV1Request {
	/// Relay parent we want a collation for.
	pub relay_parent: Hash,
	/// The `ParaId` of the collation.
	pub para_id: ParaId,
}

/// Responses as sent by collators.
#[derive(Debug, Clone, Encode, Decode)]
pub enum CollationFetchingV1Response {
	/// Deliver requested collation.
	#[codec(index = 0)]
	Collation(CandidateReceipt, PoV),
}

impl IsRequest for CollationFetchingV1Request {
	type Response = CollationFetchingV1Response;
	const PROTOCOL: Protocol = Protocol::CollationFetchingV1;
}

/// Request the advertised collation at that relay-parent.
#[derive(Debug, Clone, Encode, Decode)]
pub struct PoVFetchingV1Request {
	/// Candidate we want a PoV for.
	pub candidate_hash: CandidateHash,
}

/// Responses to `PoVFetchingV1Request`.
#[derive(Debug, Clone, Encode, Decode)]
pub enum PoVFetchingV1Response {
	/// Deliver requested PoV.
	#[codec(index = 0)]
	PoV(PoV),
	/// PoV was not found in store.
	#[codec(index = 1)]
	NoSuchPoV,
}

impl IsRequest for PoVFetchingV1Request {
	type Response = PoVFetchingV1Response;
	const PROTOCOL: Protocol = Protocol::PoVFetchingV1;
}

/// Request the entire available data for a candidate.
#[derive(Debug, Clone, Encode, Decode)]
pub struct AvailableDataFetchingV1Request {
	/// The candidate hash to get the available data for.
	pub candidate_hash: CandidateHash,
}

/// Receive a requested available data.
#[derive(Debug, Clone, Encode, Decode)]
pub enum AvailableDataFetchingV1Response {
	/// The requested data.
	#[codec(index = 0)]
	AvailableData(AvailableData),
	/// Node was not in possession of the requested data.
	#[codec(index = 1)]
	NoSuchData,
}

impl From<Option<AvailableData>> for AvailableDataFetchingV1Response {
	fn from(x: Option<AvailableData>) -> Self {
		match x {
			Some(data) => AvailableDataFetchingV1Response::AvailableData(data),
			None => AvailableDataFetchingV1Response::NoSuchData,
		}
	}
}

impl IsRequest for AvailableDataFetchingV1Request {
	type Response = AvailableDataFetchingV1Response;
	const PROTOCOL: Protocol = Protocol::AvailableDataFetchingV1;
}

/// Request for fetching a large statement via request/response.
#[derive(Debug, Clone, Encode, Decode)]
pub struct StatementFetchingV1Request {
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
pub enum StatementFetchingV1Response {
	/// Data missing to reconstruct the full signed statement.
	#[codec(index = 0)]
	Statement(CommittedCandidateReceipt),
}

impl IsRequest for StatementFetchingV1Request {
	type Response = StatementFetchingV1Response;
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
