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

use std::{
	num::NonZeroUsize,
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

use futures::{
	channel::oneshot,
	future::poll_fn,
	pin_mut,
	stream::{FuturesUnordered, StreamExt},
	Future,
};

use gum::CandidateHash;
use polkadot_node_network_protocol::{
	authority_discovery::AuthorityDiscovery,
	request_response::{
		incoming::{self, OutgoingResponse, OutgoingResponseSender},
		v1::{DisputeRequest, DisputeResponse},
		IncomingRequest, IncomingRequestReceiver,
	},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::DISPUTE_WINDOW;
use polkadot_node_subsystem::{
	messages::{DisputeCoordinatorMessage, ImportStatementsResult},
	overseer,
};
use polkadot_node_subsystem_util::{runtime, runtime::RuntimeInfo};

use crate::{
	metrics::{FAILED, SUCCEEDED},
	Metrics, LOG_TARGET,
};

mod error;

/// Rate limiting queues for incoming requests by peers.
mod peer_queues;

/// Batch imports together.
mod batches;

use self::{
	batches::{Batches, FoundBatch, PreparedImport},
	error::{log_error, JfyiError, JfyiResult, Result},
	peer_queues::PeerQueues,
};

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Received message could not be decoded.");
const COST_INVALID_SIGNATURE: Rep = Rep::Malicious("Signatures were invalid.");
const COST_INVALID_IMPORT: Rep =
	Rep::Malicious("Import was deemed invalid by dispute-coordinator.");
const COST_NOT_A_VALIDATOR: Rep = Rep::CostMajor("Reporting peer was not a validator.");
/// Mildly punish peers exceeding their rate limit.
///
/// For honest peers this should rarely happen, but if it happens we would not want to disconnect
/// too quickly. Minor cost should suffice for disconnecting any real flooder.
const COST_APPARENT_FLOOD: Rep = Rep::CostMinor("Peer exceeded the rate limit.");

/// How many votes must have arrived in the last `BATCH_COLLECTING_INTERVAL`
///
/// in order for a batch to stay alive and not get flushed/imported to the dispute-coordinator.
///
/// This ensures a timely import of batches.
#[cfg(not(test))]
pub const MIN_KEEP_BATCH_ALIVE_VOTES: u32 = 10;
#[cfg(test)]
pub const MIN_KEEP_BATCH_ALIVE_VOTES: u32 = 2;

/// Time we allow to pass for new votes to trickle in.
///
/// See `MIN_KEEP_BATCH_ALIVE_VOTES` above.
/// Should be greater or equal to `RECEIVE_RATE_LIMIT` (there is no point in checking any faster).
pub const BATCH_COLLECTING_INTERVAL: Duration = Duration::from_millis(500);

/// State for handling incoming `DisputeRequest` messages.
pub struct DisputesReceiver<Sender, AD> {
	/// Access to session information.
	runtime: RuntimeInfo,

	/// Subsystem sender for communication with other subsystems.
	sender: Sender,

	/// Channel to retrieve incoming requests from.
	receiver: IncomingRequestReceiver<DisputeRequest>,

	/// Rate limiting queue for each peer (only authorities).
	peer_queues: PeerQueues,

	/// Currently active batches of imports per candidate.
	batches: Batches,

	/// Authority discovery service:
	authority_discovery: AD,

	/// Imports currently being processed by the `dispute-coordinator`.
	pending_imports: FuturesUnordered<PendingImport>,

	/// Log received requests.
	metrics: Metrics,
}

/// Messages as handled by this receiver internally.
enum MuxedMessage {
	/// An import got confirmed by the coordinator.
	///
	/// We need to handle those for two reasons:
	///
	/// - We need to make sure responses are actually sent (therefore we need to await futures
	/// promptly).
	/// - We need to punish peers whose import got rejected.
	ConfirmedImport(ImportResult),

	/// A new request has arrived and should be handled.
	NewRequest(IncomingRequest<DisputeRequest>),

	/// Rate limit timer hit - is is time to process one row of messages.
	///
	/// This is the result of calling `self.peer_queues.pop_reqs()`.
	WakePeerQueuesPopReqs(Vec<IncomingRequest<DisputeRequest>>),

	/// It is time to check batches.
	///
	/// Every `BATCH_COLLECTING_INTERVAL` we check whether less than `MIN_KEEP_BATCH_ALIVE_VOTES`
	/// new votes arrived, if so the batch is ready for import.
	///
	/// This is the result of calling `self.batches.check_batches()`.
	WakeCheckBatches(Vec<PreparedImport>),
}

impl<Sender, AD> DisputesReceiver<Sender, AD>
where
	AD: AuthorityDiscovery,
	Sender: overseer::DisputeDistributionSenderTrait,
{
	/// Create a new receiver which can be `run`.
	pub fn new(
		sender: Sender,
		receiver: IncomingRequestReceiver<DisputeRequest>,
		authority_discovery: AD,
		metrics: Metrics,
	) -> Self {
		let runtime = RuntimeInfo::new_with_config(runtime::Config {
			keystore: None,
			session_cache_lru_size: NonZeroUsize::new(DISPUTE_WINDOW.get() as usize)
				.expect("Dispute window can not be 0; qed"),
		});
		Self {
			runtime,
			sender,
			receiver,
			peer_queues: PeerQueues::new(),
			batches: Batches::new(),
			authority_discovery,
			pending_imports: FuturesUnordered::new(),
			metrics,
		}
	}

	/// Get that receiver started.
	///
	/// This is an endless loop and should be spawned into its own task.
	pub async fn run(mut self) {
		loop {
			match log_error(self.run_inner().await) {
				Ok(()) => {},
				Err(fatal) => {
					gum::debug!(
						target: LOG_TARGET,
						error = ?fatal,
						"Shutting down"
					);
					return
				},
			}
		}
	}

	/// Actual work happening here in three phases:
	///
	/// 1. Receive and queue incoming messages until the rate limit timer hits.
	/// 2. Do import/batching for the head of all queues.
	/// 3. Check and flush any ready batches.
	async fn run_inner(&mut self) -> Result<()> {
		let msg = self.receive_message().await?;

		match msg {
			MuxedMessage::NewRequest(req) => {
				// Phase 1:
				self.metrics.on_received_request();
				self.dispatch_to_queues(req).await?;
			},
			MuxedMessage::WakePeerQueuesPopReqs(reqs) => {
				// Phase 2:
				for req in reqs {
					// No early return - we cannot cancel imports of one peer, because the import of
					// another failed:
					match log_error(self.start_import_or_batch(req).await) {
						Ok(()) => {},
						Err(fatal) => return Err(fatal.into()),
					}
				}
			},
			MuxedMessage::WakeCheckBatches(ready_imports) => {
				// Phase 3:
				self.import_ready_batches(ready_imports).await;
			},
			MuxedMessage::ConfirmedImport(import_result) => {
				self.update_imported_requests_metrics(&import_result);
				// Confirm imports to requesters/punish them on invalid imports:
				send_responses_to_requesters(import_result).await?;
			},
		}

		Ok(())
	}

	/// Receive one `MuxedMessage`.
	///
	///
	/// Dispatching events to messages as they happen.
	async fn receive_message(&mut self) -> Result<MuxedMessage> {
		poll_fn(|ctx| {
			// In case of Ready(None), we want to wait for pending requests:
			if let Poll::Ready(Some(v)) = self.pending_imports.poll_next_unpin(ctx) {
				return Poll::Ready(Ok(MuxedMessage::ConfirmedImport(v?)))
			}

			let rate_limited = self.peer_queues.pop_reqs();
			pin_mut!(rate_limited);
			// We poll rate_limit before batches, so we don't unnecessarily delay importing to
			// batches.
			if let Poll::Ready(reqs) = rate_limited.poll(ctx) {
				return Poll::Ready(Ok(MuxedMessage::WakePeerQueuesPopReqs(reqs)))
			}

			let ready_batches = self.batches.check_batches();
			pin_mut!(ready_batches);
			if let Poll::Ready(ready_batches) = ready_batches.poll(ctx) {
				return Poll::Ready(Ok(MuxedMessage::WakeCheckBatches(ready_batches)))
			}

			let next_req = self.receiver.recv(|| vec![COST_INVALID_REQUEST]);
			pin_mut!(next_req);
			if let Poll::Ready(r) = next_req.poll(ctx) {
				return match r {
					Err(e) => Poll::Ready(Err(incoming::Error::from(e).into())),
					Ok(v) => Poll::Ready(Ok(MuxedMessage::NewRequest(v))),
				}
			}
			Poll::Pending
		})
		.await
	}

	/// Process incoming requests.
	///
	/// - Check sender is authority
	/// - Dispatch message to corresponding queue in `peer_queues`.
	/// - If queue is full, drop message and change reputation of sender.
	async fn dispatch_to_queues(&mut self, req: IncomingRequest<DisputeRequest>) -> JfyiResult<()> {
		let peer = req.peer;
		// Only accept messages from validators, in case there are multiple `AuthorityId`s, we
		// just take the first one. On session boundaries this might allow validators to double
		// their rate limit for a short period of time, which seems acceptable.
		let authority_id = match self
			.authority_discovery
			.get_authority_ids_by_peer_id(peer)
			.await
			.and_then(|s| s.into_iter().next())
		{
			None => {
				req.send_outgoing_response(OutgoingResponse {
					result: Err(()),
					reputation_changes: vec![COST_NOT_A_VALIDATOR],
					sent_feedback: None,
				})
				.map_err(|_| JfyiError::SendResponses(vec![peer]))?;
				return Err(JfyiError::NotAValidator(peer).into())
			},
			Some(auth_id) => auth_id,
		};

		// Queue request:
		if let Err((authority_id, req)) = self.peer_queues.push_req(authority_id, req) {
			gum::debug!(
				target: LOG_TARGET,
				?authority_id,
				?peer,
				"Peer hit the rate limit - dropping message."
			);
			req.send_outgoing_response(OutgoingResponse {
				result: Err(()),
				reputation_changes: vec![COST_APPARENT_FLOOD],
				sent_feedback: None,
			})
			.map_err(|_| JfyiError::SendResponses(vec![peer]))?;
			return Err(JfyiError::AuthorityFlooding(authority_id))
		}
		Ok(())
	}

	/// Start importing votes for the given request or batch.
	///
	/// Signature check and in case we already have an existing batch we import to that batch,
	/// otherwise import to `dispute-coordinator` directly and open a batch.
	async fn start_import_or_batch(
		&mut self,
		incoming: IncomingRequest<DisputeRequest>,
	) -> Result<()> {
		let IncomingRequest { peer, payload, pending_response } = incoming;

		let info = self
			.runtime
			.get_session_info_by_index(
				&mut self.sender,
				payload.0.candidate_receipt.descriptor.relay_parent,
				payload.0.session_index,
			)
			.await?;

		let votes_result = payload.0.try_into_signed_votes(&info.session_info);

		let (candidate_receipt, valid_vote, invalid_vote) = match votes_result {
			Err(()) => {
				// Signature invalid:
				pending_response
					.send_outgoing_response(OutgoingResponse {
						result: Err(()),
						reputation_changes: vec![COST_INVALID_SIGNATURE],
						sent_feedback: None,
					})
					.map_err(|_| JfyiError::SetPeerReputation(peer))?;

				return Err(From::from(JfyiError::InvalidSignature(peer)))
			},
			Ok(votes) => votes,
		};

		let candidate_hash = *valid_vote.0.candidate_hash();

		match self.batches.find_batch(candidate_hash, candidate_receipt)? {
			FoundBatch::Created(batch) => {
				// There was no entry yet - start import immediately:
				gum::trace!(
					target: LOG_TARGET,
					?candidate_hash,
					?peer,
					"No batch yet - triggering immediate import"
				);
				let import = PreparedImport {
					candidate_receipt: batch.candidate_receipt().clone(),
					statements: vec![valid_vote, invalid_vote],
					requesters: vec![(peer, pending_response)],
				};
				self.start_import(import).await;
			},
			FoundBatch::Found(batch) => {
				gum::trace!(target: LOG_TARGET, ?candidate_hash, "Batch exists - batching request");
				let batch_result =
					batch.add_votes(valid_vote, invalid_vote, peer, pending_response);

				if let Err(pending_response) = batch_result {
					// We don't expect honest peers to send redundant votes within a single batch,
					// as the timeout for retry is much higher. Still we don't want to punish the
					// node as it might not be the node's fault. Some other (malicious) node could have been
					// faster sending the same votes in order to harm the reputation of that honest
					// node. Given that we already have a rate limit, if a validator chooses to
					// waste available rate with redundant votes - so be it. The actual dispute
					// resolution is unaffected.
					gum::debug!(
						target: LOG_TARGET,
						?peer,
						"Peer sent completely redundant votes within a single batch - that looks fishy!",
					);
					pending_response
						.send_outgoing_response(OutgoingResponse {
							// While we have seen duplicate votes, we cannot confirm as we don't
							// know yet whether the batch is going to be confirmed, so we assume
							// the worst. We don't want to push the pending response to the batch
							// either as that would be unbounded, only limited by the rate limit.
							result: Err(()),
							reputation_changes: Vec::new(),
							sent_feedback: None,
						})
						.map_err(|_| JfyiError::SendResponses(vec![peer]))?;
					return Err(From::from(JfyiError::RedundantMessage(peer)))
				}
			},
		}

		Ok(())
	}

	/// Trigger import into the dispute-coordinator of ready batches (`PreparedImport`s).
	async fn import_ready_batches(&mut self, ready_imports: Vec<PreparedImport>) {
		for import in ready_imports {
			self.start_import(import).await;
		}
	}

	/// Start import and add response receiver to `pending_imports`.
	async fn start_import(&mut self, import: PreparedImport) {
		let PreparedImport { candidate_receipt, statements, requesters } = import;
		let (session_index, candidate_hash) = match statements.iter().next() {
			None => {
				gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?candidate_receipt.hash(),
					"Not importing empty batch"
				);
				return
			},
			Some(vote) => (vote.0.session_index(), *vote.0.candidate_hash()),
		};

		let (pending_confirmation, confirmation_rx) = oneshot::channel();
		self.sender
			.send_message(DisputeCoordinatorMessage::ImportStatements {
				candidate_receipt,
				session: session_index,
				statements,
				pending_confirmation: Some(pending_confirmation),
			})
			.await;

		let pending =
			PendingImport { candidate_hash, requesters, pending_response: confirmation_rx };

		self.pending_imports.push(pending);
	}

	fn update_imported_requests_metrics(&self, result: &ImportResult) {
		let label = match result.result {
			ImportStatementsResult::ValidImport => SUCCEEDED,
			ImportStatementsResult::InvalidImport => FAILED,
		};
		self.metrics.on_imported(label, result.requesters.len());
	}
}

async fn send_responses_to_requesters(import_result: ImportResult) -> JfyiResult<()> {
	let ImportResult { requesters, result } = import_result;

	let mk_response = match result {
		ImportStatementsResult::ValidImport => || OutgoingResponse {
			result: Ok(DisputeResponse::Confirmed),
			reputation_changes: Vec::new(),
			sent_feedback: None,
		},
		ImportStatementsResult::InvalidImport => || OutgoingResponse {
			result: Err(()),
			reputation_changes: vec![COST_INVALID_IMPORT],
			sent_feedback: None,
		},
	};

	let mut sending_failed_for = Vec::new();
	for (peer, pending_response) in requesters {
		if let Err(()) = pending_response.send_outgoing_response(mk_response()) {
			sending_failed_for.push(peer);
		}
	}

	if !sending_failed_for.is_empty() {
		Err(JfyiError::SendResponses(sending_failed_for))
	} else {
		Ok(())
	}
}

/// A future that resolves into an `ImportResult` when ready.
///
/// This future is used on `dispute-coordinator` import messages for the oneshot response receiver
/// to:
/// - Keep track of concerned `CandidateHash` for reporting errors.
/// - Keep track of requesting peers so we can confirm the import/punish them on invalid imports.
struct PendingImport {
	candidate_hash: CandidateHash,
	requesters: Vec<(PeerId, OutgoingResponseSender<DisputeRequest>)>,
	pending_response: oneshot::Receiver<ImportStatementsResult>,
}

/// A `PendingImport` becomes an `ImportResult` once done.
struct ImportResult {
	/// Requesters of that import.
	requesters: Vec<(PeerId, OutgoingResponseSender<DisputeRequest>)>,
	/// Actual result of the import.
	result: ImportStatementsResult,
}

impl PendingImport {
	async fn wait_for_result(&mut self) -> JfyiResult<ImportResult> {
		let result = (&mut self.pending_response)
			.await
			.map_err(|_| JfyiError::ImportCanceled(self.candidate_hash))?;
		Ok(ImportResult { requesters: std::mem::take(&mut self.requesters), result })
	}
}

impl Future for PendingImport {
	type Output = JfyiResult<ImportResult>;
	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let fut = self.wait_for_result();
		pin_mut!(fut);
		fut.poll(cx)
	}
}
