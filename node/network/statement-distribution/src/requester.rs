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

//! Large statement requesting background task logic.

use std::time::Duration;

use futures::{SinkExt, channel::{mpsc, oneshot}};
use polkadot_node_network_protocol::{IfDisconnected, PeerId, request_response::{OutgoingRequest, Recipient, Requests, v1::{StatementFetchingRequest, StatementFetchingResponse}}, v1::StatementMetadata};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::{CandidateHash, CommittedCandidateReceipt, Hash, ValidatorIndex};
use polkadot_subsystem::messages::{AllMessages, NetworkBridgeMessage};

use crate::LOG_TARGET;

// In case we failed fetching from our known peers, how long we should wait before attempting a
// retry, even though we have not yet discovered any new peers. Or in other words how long to
// wait before retrying peers that already failed.
const RETRY_TIMEOUT: Duration = Duration::from_millis(500);

/// Messages coming from a background task.
pub enum RequesterMessage {
	/// Get an update of availble peers to try for fetching a given statement.
	GetMorePeers(StatementMetadata, oneshot::Sender<Vec<PeerId>>),
	/// Fetching finished.
	Finished {
		/// Response in case we got any together with the peer who provided us with it.
		response: Option<(PeerId, CommittedCandidateReceipt)>,
		/// Peers which failed providing the data - they should get punished badly in terms of
		/// reputation as they just told us a moment ago to have the data.
		bad_peers: Vec<PeerId>,
	},
	/// Ask subsystem to send a request for us.
	SendRequest(Requests),
}


/// A fetching task, taking care of fetching large statements via request/response.
///
/// Takes metadata of a statement to fetch and a list of peer ids to try for fetching. It will
/// communicate back via the given mpsc sender.
///
/// The `OverseerSubsystemSender` will be used for issuing network requests.
pub async fn fetch(
	metadata: StatementMetadata,
	peers: Vec<PeerId>,
	mut sender: mpsc::Sender<RequesterMessage>,
) {
	// Peers we already tried (and failed).
	let mut tried_peers = Vec::new();
	// Peers left for trying out.
	let mut new_peers = peers;

	let req = StatementFetchingRequest {
		relay_parent: metadata.relay_parent,
		candidate_hash: metadata.candidate_hash,
	};

	// We retry endlessly (with sleep periods), and rely on the subsystem to kill us eventually.
	loop {
		while let Some(peer) = new_peers.pop() {
			let (outgoing, pending_response) = OutgoingRequest::new(
				Recipient::Peer(peer),
				req.clone(),
			);
			// if let Err(err) = subsystem_sender
			//     .send(AllMessages::NetworkBridge(
			//         NetworkBridgeMessage::SendRequests(
			//             vec![Requests::StatementFetching(outgoing)],
			//             IfDisconnected::ImmediateError,
			//         )
			//     ))
			//     .await {
			if let Err(err) = sender.feed(
				RequesterMessage::SendRequest(Requests::StatementFetching(outgoing))
			).await {
				tracing::info!(
					target: LOG_TARGET,
					?err,
					"Sending request failed, node might be shutting down - exiting."
				);
				return
			}
			match pending_response.await {
				Ok(StatementFetchingResponse::Statement(statement)) => {
					// TODO: We absolutely have to check the signature here and punish peers for
					// which the signature won't match, we absolutely don't wan to include such a
					// statement in the cache.
					if let Err(err) = sender.send(
						RequesterMessage::Finished {
							response: Some((peer, statement)),
							bad_peers: tried_peers,
						}
						).await {
						tracing::debug!(
							target: LOG_TARGET,
							?err,
							"Sending task response failed, we might already be finished with that block."
						);
					}
					// No matter what, we are done now.
					return
				},
				Err(err) => {
					tracing::debug!(
						target: LOG_TARGET,
						?err,
						"Receiving response failed with error - trying next peer."
					);
				}
			}

			tried_peers.push(peer);
		}

		new_peers = std::mem::take(&mut tried_peers);

		// All our peers failed us - try getting new ones before trying again:
		match try_get_new_peers(metadata.clone(), &mut sender).await {
			Ok(Some(mut peers)) => {
				// New arrivals will be tried first:
				new_peers.append(&mut peers);
			}
			// No new peers, try the old ones again:
			Ok(None) => continue,
			Err(()) => return,
		}
	}
}

/// Try getting new peers from subsystem.
///
/// If there are non, we will return after a timeout with `None`.
async fn try_get_new_peers(
	fingerprint: StatementMetadata,
	sender: &mut mpsc::Sender<RequesterMessage>
) -> Result<Option<Vec<PeerId>>, ()> {
	let (tx, rx) = oneshot::channel();

	if let Err(err) = sender.send(
		RequesterMessage::GetMorePeers(fingerprint, tx)
	).await {
		tracing::debug!(
			target: LOG_TARGET,
			?err,
			"Failed sending background task message, subsystem probably moved on."
		);
		return Err(())
	}

	match rx.timeout(RETRY_TIMEOUT).await.transpose() {
		Err(_) => {
			tracing::debug!(
				target: LOG_TARGET,
				"Failed fetching more peers."
			);
			Err(())
		}
		Ok(val) => Ok(val)
	}
}

