// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Large statement responding background task logic.

use futures::{SinkExt, StreamExt, channel::{mpsc, oneshot}, stream::FuturesUnordered};

use parity_scale_codec::Decode;

use polkadot_node_network_protocol::{
	PeerId, UnifiedReputationChange as Rep,
	request_response::{
		IncomingRequest, MAX_PARALLEL_STATEMENT_REQUESTS, request::OutgoingResponse,
		v1::{
			StatementFetchingRequest, StatementFetchingResponse
		},
	},
};
use polkadot_primitives::v1::{CandidateHash, CommittedCandidateReceipt, Hash};

use crate::LOG_TARGET;

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Peer sent unparsable request");

/// Messages coming from a background task.
pub enum ResponderMessage {
	/// Get an update of availble peers to try for fetching a given statement.
	GetData {
		requesting_peer: PeerId,
		relay_parent: Hash,
		candidate_hash: CandidateHash,
		tx: oneshot::Sender<CommittedCandidateReceipt>
	},
}


/// A fetching task, taking care of fetching large statements via request/response.
///
/// A fetch task does not know about a particular `Statement` instead it just tries fetching a
/// `CommittedCandidateReceipt` from peers, whether or not this can be used to re-assemble one ore
/// many `SignedFullStatement`s needs to be verified by the caller.
pub async fn respond(
	mut receiver: mpsc::Receiver<sc_network::config::IncomingRequest>,
	mut sender: mpsc::Sender<ResponderMessage>,
) {
	let mut pending_out = FuturesUnordered::new();
	loop {
		// Ensure we are not handling too many requests in parallel.
		// We do this for three reasons:
		//
		// 1. We want some requesters to have full data fast, rather then lots of them having them
		//    late, as each requester having the data will help distributing it.
		// 2. If we take too long, the requests timing out will not yet have had any data sent,
		//    thus we wasted no bandwidth.
		// 3. If the queue is full, requestes will get an immediate error instead of running in a
		//    timeout, thus requesters can immediately try another peer and be faster.
		//
		// From this perspective we would not want parallel response sending at all, but we don't
		// want a single slow requester slowing everyone down, so we want some parallelism for that
		// reason.
		if pending_out.len() >= MAX_PARALLEL_STATEMENT_REQUESTS as usize {
			// Wait for one to finish:
			pending_out.next().await;
		}

		let raw = match receiver.next().await {
			None => {
				tracing::debug!(
					target: LOG_TARGET,
					"Shutting down request responder"
				);
				return
			}
			Some(v) => v,
		};

		let sc_network::config::IncomingRequest {
			payload,
			peer,
			pending_response,
		} = raw;

		let payload = match StatementFetchingRequest::decode(&mut payload.as_ref()) {
			Err(err) => {
				tracing::debug!(
					target: LOG_TARGET,
					?err,
					"Decoding request failed"
				);
				report_peer(pending_response, COST_INVALID_REQUEST);
				continue
			}
			Ok(payload) => payload,
		};

		let req = IncomingRequest::new(
			peer,
			payload,
			pending_response
		);

		let (tx, rx) = oneshot::channel();
		if let Err(err) = sender.feed(
			ResponderMessage::GetData {
				requesting_peer: peer,
				relay_parent: req.payload.relay_parent,
				candidate_hash: req.payload.candidate_hash,
				tx,
			}
		).await {
			tracing::debug!(
				target: LOG_TARGET,
				?err,
				"Shutting down responder"
			);
			return
		}
		let response = match rx.await {
			Err(err) => {
				tracing::debug!(
					target: LOG_TARGET,
					?err,
					"Requested data not found."
				);
				Err(())
			}
			Ok(v) => Ok(StatementFetchingResponse::Statement(v)),
		};
		let (pending_sent_tx, pending_sent_rx) = oneshot::channel();
		let response = OutgoingResponse {
			result: response,
			reputation_changes: Vec::new(),
			sent_feedback: Some(pending_sent_tx),
		};
		pending_out.push(pending_sent_rx);
		if let Err(_) = req.send_outgoing_response(response) {
			tracing::debug!(
				target: LOG_TARGET,
				"Sending response failed"
			);
		}
	}
}

/// Report peer who sent us a request.
fn report_peer(
	tx: oneshot::Sender<sc_network::config::OutgoingResponse>,
	rep: Rep,
) {
	if let Err(_) = tx.send(sc_network::config::OutgoingResponse {
		result: Err(()),
		reputation_changes: vec![rep.into_base_rep()],
		sent_feedback: None,
	}) {
		tracing::debug!(
			target: LOG_TARGET,
			"Reporting peer failed."
		);
	}
}
