// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Large statement requesting background task logic.

use std::time::Duration;

use futures::{
	channel::{mpsc, oneshot},
	SinkExt,
};

use polkadot_node_network_protocol::{
	request_response::{
		v1::{StatementFetchingRequest, StatementFetchingResponse},
		OutgoingRequest, Recipient, Requests,
	},
	PeerId, UnifiedReputationChange,
};
use polkadot_node_subsystem::{Span, Stage};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::{CandidateHash, CommittedCandidateReceipt, Hash};

use crate::{
	legacy_v1::{COST_WRONG_HASH, LOG_TARGET},
	metrics::Metrics,
};

// In case we failed fetching from our known peers, how long we should wait before attempting a
// retry, even though we have not yet discovered any new peers. Or in other words how long to
// wait before retrying peers that already failed.
const RETRY_TIMEOUT: Duration = Duration::from_millis(500);

/// Messages coming from a background task.
pub enum RequesterMessage {
	/// Get an update of available peers to try for fetching a given statement.
	GetMorePeers {
		relay_parent: Hash,
		candidate_hash: CandidateHash,
		tx: oneshot::Sender<Vec<PeerId>>,
	},
	/// Fetching finished, ask for verification. If verification fails, task will continue asking
	/// peers for data.
	Finished {
		/// Relay parent this candidate is in the context of.
		relay_parent: Hash,
		/// The candidate we fetched data for.
		candidate_hash: CandidateHash,
		/// Data was fetched from this peer.
		from_peer: PeerId,
		/// Response we received from above peer.
		response: CommittedCandidateReceipt,
		/// Peers which failed providing the data.
		bad_peers: Vec<PeerId>,
	},
	/// Report a peer which behaved worse than just not providing data:
	ReportPeer(PeerId, UnifiedReputationChange),
	/// Ask subsystem to send a request for us.
	SendRequest(Requests),
}

/// A fetching task, taking care of fetching large statements via request/response.
///
/// A fetch task does not know about a particular `Statement` instead it just tries fetching a
/// `CommittedCandidateReceipt` from peers, whether this can be used to re-assemble one ore
/// many `SignedFullStatement`s needs to be verified by the caller.
pub async fn fetch(
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	peers: Vec<PeerId>,
	mut sender: mpsc::Sender<RequesterMessage>,
	metrics: Metrics,
) {
	let span = Span::new(candidate_hash, "fetch-large-statement")
		.with_relay_parent(relay_parent)
		.with_stage(Stage::StatementDistribution);

	gum::debug!(
		target: LOG_TARGET,
		?candidate_hash,
		?relay_parent,
		"Fetch for large statement started",
	);

	// Peers we already tried (and failed).
	let mut tried_peers = Vec::new();
	// Peers left for trying out.
	let mut new_peers = peers;

	let req = StatementFetchingRequest { relay_parent, candidate_hash };

	// We retry endlessly (with sleep periods), and rely on the subsystem to kill us eventually.
	loop {
		let span = span.child("try-available-peers");

		while let Some(peer) = new_peers.pop() {
			let _span = span.child("try-peer").with_peer_id(&peer);

			let (outgoing, pending_response) =
				OutgoingRequest::new(Recipient::Peer(peer), req.clone());
			if let Err(err) = sender
				.feed(RequesterMessage::SendRequest(Requests::StatementFetchingV1(outgoing)))
				.await
			{
				gum::info!(
					target: LOG_TARGET,
					?err,
					"Sending request failed, node might be shutting down - exiting."
				);
				return
			}

			metrics.on_sent_request();

			match pending_response.await {
				Ok(StatementFetchingResponse::Statement(statement)) => {
					if statement.hash() != candidate_hash {
						metrics.on_received_response(false);
						metrics.on_unexpected_statement_large();

						if let Err(err) =
							sender.feed(RequesterMessage::ReportPeer(peer, COST_WRONG_HASH)).await
						{
							gum::warn!(
								target: LOG_TARGET,
								?err,
								"Sending reputation change failed: This should not happen."
							);
						}
						// We want to get rid of this peer:
						continue
					}

					if let Err(err) = sender
						.feed(RequesterMessage::Finished {
							relay_parent,
							candidate_hash,
							from_peer: peer,
							response: statement,
							bad_peers: tried_peers,
						})
						.await
					{
						gum::warn!(
							target: LOG_TARGET,
							?err,
							"Sending task response failed: This should not happen."
						);
					}

					metrics.on_received_response(true);

					// We are done now.
					return
				},
				Err(err) => {
					gum::debug!(
						target: LOG_TARGET,
						?err,
						"Receiving response failed with error - trying next peer."
					);

					metrics.on_received_response(false);
					metrics.on_unexpected_statement_large();
				},
			}

			tried_peers.push(peer);
		}

		new_peers = std::mem::take(&mut tried_peers);

		// All our peers failed us - try getting new ones before trying again:
		match try_get_new_peers(relay_parent, candidate_hash, &mut sender, &span).await {
			Ok(Some(mut peers)) => {
				gum::trace!(target: LOG_TARGET, ?peers, "Received new peers.");
				// New arrivals will be tried first:
				new_peers.append(&mut peers);
			},
			// No new peers, try the old ones again (if we have any):
			Ok(None) => {
				// Note: In case we don't have any more peers, we will just keep asking for new
				// peers, which is exactly what we want.
			},
			Err(()) => return,
		}
	}
}

/// Try getting new peers from subsystem.
///
/// If there are non, we will return after a timeout with `None`.
async fn try_get_new_peers(
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	sender: &mut mpsc::Sender<RequesterMessage>,
	span: &Span,
) -> Result<Option<Vec<PeerId>>, ()> {
	let _span = span.child("wait-for-peers");

	let (tx, rx) = oneshot::channel();

	if let Err(err) = sender
		.send(RequesterMessage::GetMorePeers { relay_parent, candidate_hash, tx })
		.await
	{
		gum::debug!(
			target: LOG_TARGET,
			?err,
			"Failed sending background task message, subsystem probably moved on."
		);
		return Err(())
	}

	match rx.timeout(RETRY_TIMEOUT).await.transpose() {
		Err(_) => {
			gum::debug!(target: LOG_TARGET, "Failed fetching more peers.");
			Err(())
		},
		Ok(val) => Ok(val),
	}
}
