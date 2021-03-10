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

//! Answer requests for availability chunks.

use futures::channel::oneshot;

use polkadot_node_network_protocol::request_response::{request::IncomingRequest, v1};
use polkadot_primitives::v1::{CandidateHash, ErasureChunk, ValidatorIndex};
use polkadot_subsystem::{
	messages::{AllMessages, AvailabilityStoreMessage},
	SubsystemContext, jaeger,
};

use crate::error::{Error, Result};
use crate::{LOG_TARGET, metrics::{Metrics, SUCCEEDED, FAILED, NOT_FOUND}};

/// Variant of `answer_request` that does Prometheus metric and logging on errors.
///
/// Any errors of `answer_request` will simply be logged.
pub async fn answer_request_log<Context>(
	ctx: &mut Context,
	req: IncomingRequest<v1::AvailabilityFetchingRequest>,
	metrics: &Metrics,
) -> ()
where
	Context: SubsystemContext,
{
	let res = answer_request(ctx, req).await;
	match res {
		Ok(result) =>
			metrics.on_served(if result {SUCCEEDED} else {NOT_FOUND}),
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				err= ?err,
				"Serving chunk failed with error"
			);
			metrics.on_served(FAILED);
		}
	}
}

/// Answer an incoming chunk request by querying the av store.
///
/// Returns: Ok(true) if chunk was found and served.
pub async fn answer_request<Context>(
	ctx: &mut Context,
	req: IncomingRequest<v1::AvailabilityFetchingRequest>,
) -> Result<bool>
where
	Context: SubsystemContext,
{
	let mut span = jaeger::candidate_hash_span(&req.payload.candidate_hash, "answer-request");
	span.add_stage(jaeger::Stage::AvailabilityDistribution);
	let _child_span = span.child_builder("answer-chunk-request")
		.with_chunk_index(req.payload.index.0)
		.build();

	let chunk = query_chunk(ctx, req.payload.candidate_hash, req.payload.index).await?;

	let result = chunk.is_some();

	let response = match chunk {
		None => v1::AvailabilityFetchingResponse::NoSuchChunk,
		Some(chunk) => v1::AvailabilityFetchingResponse::Chunk(chunk.into()),
	};

	req.send_response(response).map_err(|_| Error::SendResponse)?;
	Ok(result)
}

/// Query chunk from the availability store.
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
