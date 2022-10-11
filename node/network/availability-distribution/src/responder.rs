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

use std::sync::Arc;

use futures::channel::oneshot;

use fatality::Nested;
use polkadot_node_network_protocol::{
	request_response::{v1, IncomingRequest, IncomingRequestReceiver},
	UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::{AvailableData, ErasureChunk};
use polkadot_node_subsystem::{jaeger, messages::AvailabilityStoreMessage, SubsystemSender};
use polkadot_primitives::v2::{CandidateHash, ValidatorIndex};

use crate::{
	error::{JfyiError, Result},
	metrics::{Metrics, FAILED, NOT_FOUND, SUCCEEDED},
	LOG_TARGET,
};

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Received message could not be decoded.");

/// Receiver task to be forked as a separate task to handle PoV requests.
pub async fn run_pov_receiver<Sender>(
	mut sender: Sender,
	mut receiver: IncomingRequestReceiver<v1::PoVFetchingRequest>,
	metrics: Metrics,
) where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	loop {
		match receiver.recv(|| vec![COST_INVALID_REQUEST]).await.into_nested() {
			Ok(Ok(msg)) => {
				answer_pov_request_log(&mut sender, msg, &metrics).await;
			},
			Err(fatal) => {
				gum::debug!(
					target: LOG_TARGET,
					error = ?fatal,
					"Shutting down POV receiver."
				);
				return
			},
			Ok(Err(jfyi)) => {
				gum::debug!(target: LOG_TARGET, error = ?jfyi, "Error decoding incoming PoV request.");
			},
		}
	}
}

/// Receiver task to be forked as a separate task to handle chunk requests.
pub async fn run_chunk_receiver<Sender>(
	mut sender: Sender,
	mut receiver: IncomingRequestReceiver<v1::ChunkFetchingRequest>,
	metrics: Metrics,
) where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	loop {
		match receiver.recv(|| vec![COST_INVALID_REQUEST]).await.into_nested() {
			Ok(Ok(msg)) => {
				answer_chunk_request_log(&mut sender, msg, &metrics).await;
			},
			Err(fatal) => {
				gum::debug!(
					target: LOG_TARGET,
					error = ?fatal,
					"Shutting down chunk receiver."
				);
				return
			},
			Ok(Err(jfyi)) => {
				gum::debug!(
					target: LOG_TARGET,
					error = ?jfyi,
					"Error decoding incoming chunk request."
				);
			},
		}
	}
}

/// Variant of `answer_pov_request` that does Prometheus metric and logging on errors.
///
/// Any errors of `answer_pov_request` will simply be logged.
pub async fn answer_pov_request_log<Sender>(
	sender: &mut Sender,
	req: IncomingRequest<v1::PoVFetchingRequest>,
	metrics: &Metrics,
) where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let res = answer_pov_request(sender, req).await;
	match res {
		Ok(result) => metrics.on_served_pov(if result { SUCCEEDED } else { NOT_FOUND }),
		Err(err) => {
			gum::warn!(
				target: LOG_TARGET,
				err= ?err,
				"Serving PoV failed with error"
			);
			metrics.on_served_pov(FAILED);
		},
	}
}

/// Variant of `answer_chunk_request` that does Prometheus metric and logging on errors.
///
/// Any errors of `answer_request` will simply be logged.
pub async fn answer_chunk_request_log<Sender>(
	sender: &mut Sender,
	req: IncomingRequest<v1::ChunkFetchingRequest>,
	metrics: &Metrics,
) -> ()
where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let res = answer_chunk_request(sender, req).await;
	match res {
		Ok(result) => metrics.on_served_chunk(if result { SUCCEEDED } else { NOT_FOUND }),
		Err(err) => {
			gum::warn!(
				target: LOG_TARGET,
				err= ?err,
				"Serving chunk failed with error"
			);
			metrics.on_served_chunk(FAILED);
		},
	}
}

/// Answer an incoming PoV fetch request by querying the av store.
///
/// Returns: `Ok(true)` if chunk was found and served.
pub async fn answer_pov_request<Sender>(
	sender: &mut Sender,
	req: IncomingRequest<v1::PoVFetchingRequest>,
) -> Result<bool>
where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let _span = jaeger::Span::new(req.payload.candidate_hash, "answer-pov-request");

	let av_data = query_available_data(sender, req.payload.candidate_hash).await?;

	let result = av_data.is_some();

	let response = match av_data {
		None => v1::PoVFetchingResponse::NoSuchPoV,
		Some(av_data) => {
			let pov = Arc::try_unwrap(av_data.pov).unwrap_or_else(|a| (&*a).clone());
			v1::PoVFetchingResponse::PoV(pov)
		},
	};

	req.send_response(response).map_err(|_| JfyiError::SendResponse)?;
	Ok(result)
}

/// Answer an incoming chunk request by querying the av store.
///
/// Returns: `Ok(true)` if chunk was found and served.
pub async fn answer_chunk_request<Sender>(
	sender: &mut Sender,
	req: IncomingRequest<v1::ChunkFetchingRequest>,
) -> Result<bool>
where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let span = jaeger::Span::new(req.payload.candidate_hash, "answer-chunk-request");

	let _child_span = span.child("answer-chunk-request").with_chunk_index(req.payload.index.0);

	let chunk = query_chunk(sender, req.payload.candidate_hash, req.payload.index).await?;

	let result = chunk.is_some();

	gum::trace!(
		target: LOG_TARGET,
		hash = ?req.payload.candidate_hash,
		index = ?req.payload.index,
		peer = ?req.peer,
		has_data = ?chunk.is_some(),
		"Serving chunk",
	);

	let response = match chunk {
		None => v1::ChunkFetchingResponse::NoSuchChunk,
		Some(chunk) => v1::ChunkFetchingResponse::Chunk(chunk.into()),
	};

	req.send_response(response).map_err(|_| JfyiError::SendResponse)?;
	Ok(result)
}

/// Query chunk from the availability store.
async fn query_chunk<Sender>(
	sender: &mut Sender,
	candidate_hash: CandidateHash,
	validator_index: ValidatorIndex,
) -> std::result::Result<Option<ErasureChunk>, JfyiError>
where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx).into(),
		)
		.await;

	let result = rx.await.map_err(|e| {
		gum::trace!(
			target: LOG_TARGET,
			?validator_index,
			?candidate_hash,
			error = ?e,
			"Error retrieving chunk",
		);
		JfyiError::QueryChunkResponseChannel(e)
	})?;
	Ok(result)
}

/// Query PoV from the availability store.
async fn query_available_data<Sender>(
	sender: &mut Sender,
	candidate_hash: CandidateHash,
) -> Result<Option<AvailableData>>
where
	Sender: SubsystemSender<AvailabilityStoreMessage>,
{
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx).into())
		.await;

	let result = rx.await.map_err(JfyiError::QueryAvailableDataResponseChannel)?;
	Ok(result)
}
