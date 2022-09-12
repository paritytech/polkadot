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

use futures::{
	channel::{mpsc, oneshot},
	stream::FuturesUnordered,
	SinkExt, StreamExt,
};

use fatality::Nested;
use polkadot_node_network_protocol::{
	request_response::{
		incoming::OutgoingResponse,
		v1::{StatementFetchingRequest, StatementFetchingResponse},
		IncomingRequestReceiver, MAX_PARALLEL_STATEMENT_REQUESTS,
	},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::v2::{CandidateHash, CommittedCandidateReceipt, Hash};

use crate::LOG_TARGET;

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Peer sent unparsable request");

/// Messages coming from a background task.
pub enum ResponderMessage {
	/// Get an update of available peers to try for fetching a given statement.
	GetData {
		requesting_peer: PeerId,
		relay_parent: Hash,
		candidate_hash: CandidateHash,
		tx: oneshot::Sender<CommittedCandidateReceipt>,
	},
}

/// A fetching task, taking care of fetching large statements via request/response.
///
/// A fetch task does not know about a particular `Statement` instead it just tries fetching a
/// `CommittedCandidateReceipt` from peers, whether this can be used to re-assemble one ore
/// many `SignedFullStatement`s needs to be verified by the caller.
pub async fn respond(
	mut receiver: IncomingRequestReceiver<StatementFetchingRequest>,
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

		let req = match receiver.recv(|| vec![COST_INVALID_REQUEST]).await.into_nested() {
			Ok(Ok(v)) => v,
			Err(fatal) => {
				gum::debug!(target: LOG_TARGET, error = ?fatal, "Shutting down request responder");
				return
			},
			Ok(Err(jfyi)) => {
				gum::debug!(target: LOG_TARGET, error = ?jfyi, "Decoding request failed");
				continue
			},
		};

		let (tx, rx) = oneshot::channel();
		if let Err(err) = sender
			.feed(ResponderMessage::GetData {
				requesting_peer: req.peer,
				relay_parent: req.payload.relay_parent,
				candidate_hash: req.payload.candidate_hash,
				tx,
			})
			.await
		{
			gum::debug!(target: LOG_TARGET, ?err, "Shutting down responder");
			return
		}
		let response = match rx.await {
			Err(err) => {
				gum::debug!(target: LOG_TARGET, ?err, "Requested data not found.");
				Err(())
			},
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
			gum::debug!(target: LOG_TARGET, "Sending response failed");
		}
	}
}
