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

use std::{collections::HashMap, time::Duration};


use futures::{SinkExt, channel::{mpsc, oneshot}};
use thiserror::Error;

use polkadot_node_network_protocol::{IfDisconnected, PeerId, request_response::{OutgoingRequest, Recipient, Requests, v1::{StatementFetchingRequest, StatementFetchingResponse}}, v1::StatementMetadata};
use polkadot_node_primitives::{SignedFullStatement, Statement};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::{CandidateHash, CommittedCandidateReceipt, Hash, SessionIndex, SigningContext, ValidatorId, ValidatorIndex};
use polkadot_subsystem::messages::{AllMessages, NetworkBridgeMessage};

use crate::{ActiveHeadData, LOG_TARGET};

// In case we failed fetching from our known peers, how long we should wait before attempting a
// retry, even though we have not yet discovered any new peers. Or in other words how long to
// wait before retrying peers that already failed.
const RETRY_TIMEOUT: Duration = Duration::from_millis(500);

/// Messages coming from a background task.
pub enum RequesterMessage {
	/// Get an update of availble peers to try for fetching a given statement.
	GetMorePeers(Hash, CandidateHash, oneshot::Sender<Vec<PeerId>>),
	/// Fetching finished, ask for verification. If verification failes, task will continue asking
	/// peers for data.
	Verify {
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
		/// Tell requester task whether or not the fetched data could be verified successfully or
		/// not. If not, the task should continue requesting for `CommittedCandidateReceipt`s from
		/// peers.
		is_ok: oneshot::Sender<bool>,
	},
	/// Ask subsystem to send a request for us.
	SendRequest(Requests),
}


/// A fetching task, taking care of fetching large statements via request/response.
///
/// A fetch task does not know about a particular `Statement` instead it just tries fetching a
/// `CommittedCandidateReceipt` from peers, whether or not this can be used to re-assemble one ore
/// many `SignedFullStatement`s needs to be verified by the caller.
pub async fn fetch(
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	peers: Vec<PeerId>,
	mut sender: mpsc::Sender<RequesterMessage>,
) {
	// Peers we already tried (and failed).
	let mut tried_peers = Vec::new();
	// Peers left for trying out.
	let mut new_peers = peers;

	let req = StatementFetchingRequest {
		relay_parent,
		candidate_hash,
	};

	// We retry endlessly (with sleep periods), and rely on the subsystem to kill us eventually.
	loop {
		while let Some(peer) = new_peers.pop() {
			let (outgoing, pending_response) = OutgoingRequest::new(
				Recipient::Peer(peer),
				req.clone(),
			);
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
					let (is_ok_tx, is_ok) = oneshot::channel();
					if let Err(err) = sender.send(
						RequesterMessage::Verify {
							relay_parent,
							candidate_hash,
							from_peer: peer,
							response: statement,
							bad_peers: tried_peers.clone(),
							is_ok: is_ok_tx,
						}
						).await {
						tracing::info!(
							target: LOG_TARGET,
							?err,
							"Sending task response failed: This should not happen."
						);
					}
					match is_ok.await {
						Err(_) => {
							tracing::debug!(
								target: LOG_TARGET,
								"No verification result, relay parent already finished?"
							);
						}
						// Verification failed => try next peer.
						Ok(false) => continue,
						Ok(true) => {},
					}
					// We are done now.
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
		match try_get_new_peers(relay_parent, candidate_hash, &mut sender).await {
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

/// Possible errors of `build_signed_full_statement`.
#[derive(Debug, Copy, Clone, Error)]
pub enum BuildStatementError {
	/// Signature has been invalid.
	#[error("Invalid signature")]
	InvalidSignature,
	/// `ValidatorIndex` in metadata could not be found.
	#[error("Signing validator could not be found")]
	UnknownValidator,
}


/// Result of `build_signed_full_statement`.
pub type BuildStatementResult = Result<SignedFullStatement, BuildStatementError>;

/// Try rebuilding a signed full statement.
///
/// By passing in the original meta data and the `CommittedCandidateReceipt` as fetched by the
/// requester. 
pub fn build_signed_full_statement<'a, F>(
	session_index: SessionIndex,
	get_validator_id: F,
	metadata: &StatementMetadata,
	receipt: CommittedCandidateReceipt
) -> BuildStatementResult 
	where
		F: FnOnce(ValidatorIndex) -> Option<&'a ValidatorId> + 'a
{
	let signing_context = SigningContext {
		session_index,
		parent_hash: metadata.relay_parent,
	};

	let validator_id =
		get_validator_id(metadata.signed_by)
			.ok_or(BuildStatementError::UnknownValidator)?;

	SignedFullStatement::new(
		Statement::Seconded(receipt),
		metadata.signed_by,
		metadata.signature.clone(),
		&signing_context,
		validator_id,
	)
	.ok_or(BuildStatementError::InvalidSignature)
}


/// Try getting new peers from subsystem.
///
/// If there are non, we will return after a timeout with `None`.
async fn try_get_new_peers(
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	sender: &mut mpsc::Sender<RequesterMessage>
) -> Result<Option<Vec<PeerId>>, ()> {
	let (tx, rx) = oneshot::channel();

	if let Err(err) = sender.send(
		RequesterMessage::GetMorePeers(relay_parent, candidate_hash, tx)
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

