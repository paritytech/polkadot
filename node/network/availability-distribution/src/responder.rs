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

//! Responder answers requests for availability chunks.

use futures::channel::oneshot;

use polkadot_node_network_protocol::request_response::{request::IncomingRequest, v1};
use polkadot_primitives::v1::{CandidateHash, ErasureChunk, ValidatorIndex};
use polkadot_subsystem::{
	messages::{AllMessages, AvailabilityStoreMessage},
	SubsystemContext,
};

use crate::error::{Error, Result};
use crate::LOG_TARGET;

/// Answer an incoming chunk request by querying the av store.
pub async fn answer_request<Context>(
	ctx: &mut Context,
	req: IncomingRequest<v1::AvailabilityFetchingRequest>,
) -> Result<()>
where
	Context: SubsystemContext,
{
	let chunk = query_chunk(ctx, req.payload.candidate_hash, req.payload.index).await?;

	let response = match chunk {
		None => v1::AvailabilityFetchingResponse::NoSuchChunk,
		Some(chunk) => v1::AvailabilityFetchingResponse::Chunk(chunk.into()),
	};

	req.send_response(response).map_err(|_| Error::SendResponse)
}

#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_chunk<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
	validator_index: ValidatorIndex,
) -> Result<Option<ErasureChunk>>
where
	Context: SubsystemContext,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx),
	))
	.await;

	rx.await.map_err(|e| Error::QueryChunkResponseChannel(e))
}
