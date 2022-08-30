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
	collections::HashSet,
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

use futures::{
	channel::oneshot,
	future::{poll_fn, BoxFuture},
	pin_mut,
	stream::{FusedStream, FuturesUnordered, StreamExt},
	Future, FutureExt, Stream,
};

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
const COST_INVALID_CANDIDATE: Rep = Rep::Malicious("Reported candidate was not available.");
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
/// This ensures a timely import once of batches.
pub const MIN_KEEP_BATCH_ALIVE_VOTES: u32 = 10;

/// Time we allow to pass for new votes to trickle in.
///
/// See `MIN_KEEP_BATCH_ALIVE_VOTES` above. Must be greater or equal to `RECEIVE_RATE_LIMIT`.
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

	/// Imports currently being processed.
	///
	/// TODO: Flush batches on invalid result of first vote import.
	pending_imports: PendingImports,

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
	/// - We need to update `banned_peers` accordingly to the result.
	ConfirmedImport(JfyiResult<(PeerId, ImportStatementsResult)>),

	/// A new request has arrived and should be handled.
	NewRequest(IncomingRequest<DisputeRequest>),

	/// Rate limit timer hit - is is time to process one row of messages.
	///
	/// This is the result of calling self.peer_queues.pop_reqs().
	WakePeerQueuesPopReqs(Vec<IncomingRequest<DisputeRequest>>),

	/// It is time to check batches.
	///
	/// Every `BATCH_COLLECTING_INTERVAL` we check whether less than `MIN_KEEP_BATCH_ALIVE_VOTES`
	/// new votes arrived, if so the batch is ready for import.
	///
	/// This is the result of calling self.batches.check_batches().
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
			session_cache_lru_size: DISPUTE_WINDOW.get() as usize,
		});
		Self {
			runtime,
			sender,
			receiver,
			peer_queues: PeerQueues::new(),
			batches: Batches::new(),
			authority_discovery,
			pending_imports: PendingImports::new(),
			// Size of MAX_PARALLEL_IMPORTS ensures we are going to immediately get rid of any
			// malicious requests still pending in the incoming queue.
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
				self.import_ready_batches(ready_imports).await?;
			},
			MuxedMessage::ConfirmedImport(m_bad) => {
				// Handle import confirmation:
				self.ban_bad_peer(m_bad)?;
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
				return Poll::Ready(Ok(MuxedMessage::ConfirmedImport(v)))
			}

			let rate_limited = self.peer_queues.pop_reqs();
			pin_mut!(rate_limited);
			// We poll rate_limit before batches, so we don't unecessarily delay importing to
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
				.map_err(|_| JfyiError::SendResponse(peer))?;
				return Err(JfyiError::NotAValidator(peer).into())
			},
			Some(auth_id) => auth_id,
		};

		// Queue request:
		if let Err((authority_id, req)) = self.peer_queues.push_req(authority_id, req) {
			req.send_outgoing_response(OutgoingResponse {
				result: Err(()),
				reputation_changes: vec![COST_APPARENT_FLOOD],
				sent_feedback: None,
			})
			.map_err(|_| JfyiError::SendResponse(peer))?;
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

		match self.batches.find_batch(*valid_vote.0.candidate_hash(), candidate_receipt) {
			FoundBatch::Created(batch) => {
				// There was no entry yet - start import immediately:
				let (pending_confirmation, confirmation_rx) = oneshot::channel();
				self.sender
					.send_message(DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: batch.candidate_receipt().clone(),
						session: valid_vote.0.session_index(),
						statements: vec![valid_vote, invalid_vote],
						pending_confirmation: Some(pending_confirmation),
					})
					.await;

				self.pending_imports.push(peer, confirmation_rx, pending_response);
			},
			FoundBatch::Found(batch) => {
				let batch_result = batch.add_votes(valid_vote, invalid_vote, pending_response);

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
						.map_err(|_| JfyiError::SendResponse(peer))?;
					return Err(From::from(JfyiError::RedundantMessage(peer)))
				}
			},
		}

		Ok(())
	}

	/// Trigger import into the dispute-coordinator of ready batches (`PreparedImport`s).
	async fn import_ready_batches(&mut self, ready_imports: Vec<PreparedImport>) -> Result<()> {
		for import in ready_imports {
			let PreparedImport { candidate_receipt, statements, pending_responses } = import;
			let session_index = match statements.iter().next() {
				None => {
					gum::debug!(
						target: LOG_TARGET,
						candidate_hash = ?candidate_receipt.hash(),
						"Not importing empty batch"
					);
					continue
				},
				Some(vote) => vote.0.session_index(),
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
			// TODO:
			//	Confirmation has to trigger response senders:
		}
		unimplemented!("WIP")
	}

	/// Await an import and ban any misbehaving peers.
	///
	/// In addition we report import metrics.
	fn ban_bad_peer(
		&mut self,
		result: JfyiResult<(PeerId, ImportStatementsResult)>,
	) -> JfyiResult<()> {
		match result? {
			(_, ImportStatementsResult::ValidImport) => {
				self.metrics.on_imported(SUCCEEDED);
			},
			(bad_peer, ImportStatementsResult::InvalidImport) => {
				self.metrics.on_imported(FAILED);
				self.banned_peers.put(bad_peer, ());
			},
		}
		Ok(())
	}
}

/// Manage pending imports in a way that preserves invariants.
struct PendingImports {
	/// Futures in flight.
	futures: FuturesUnordered<BoxFuture<'static, (PeerId, JfyiResult<ImportStatementsResult>)>>,
	/// Peers whose requests are currently in flight.
	peers: HashSet<PeerId>,
}

impl PendingImports {
	pub fn new() -> Self {
		Self { futures: FuturesUnordered::new(), peers: HashSet::new() }
	}

	pub fn push(
		&mut self,
		peer: PeerId,
		handled: oneshot::Receiver<ImportStatementsResult>,
		pending_response: OutgoingResponseSender<DisputeRequest>,
	) {
		self.peers.insert(peer);
		self.futures.push(
			async move {
				let r = respond_to_request(peer, handled, pending_response).await;
				(peer, r)
			}
			.boxed(),
		)
	}

	/// Returns the number of contained futures.
	pub fn len(&self) -> usize {
		self.futures.len()
	}

	/// Check whether a peer has a pending import.
	pub fn peer_is_pending(&self, peer: &PeerId) -> bool {
		self.peers.contains(peer)
	}
}

impl Stream for PendingImports {
	type Item = JfyiResult<(PeerId, ImportStatementsResult)>;
	fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		match Pin::new(&mut self.futures).poll_next(ctx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Ready(Some((peer, result))) => {
				self.peers.remove(&peer);
				Poll::Ready(Some(result.map(|r| (peer, r))))
			},
		}
	}
}
impl FusedStream for PendingImports {
	fn is_terminated(&self) -> bool {
		self.futures.is_terminated()
	}
}

// Future for `PendingImports`
//
// - Wait for import
// - Punish peer
// - Deliver result
async fn respond_to_request(
	peer: PeerId,
	handled: oneshot::Receiver<ImportStatementsResult>,
	pending_response: OutgoingResponseSender<DisputeRequest>,
) -> JfyiResult<ImportStatementsResult> {
	let result = handled.await.map_err(|_| JfyiError::ImportCanceled(peer))?;

	let response = match result {
		ImportStatementsResult::ValidImport => OutgoingResponse {
			result: Ok(DisputeResponse::Confirmed),
			reputation_changes: Vec::new(),
			sent_feedback: None,
		},
		ImportStatementsResult::InvalidImport => OutgoingResponse {
			result: Err(()),
			reputation_changes: vec![COST_INVALID_CANDIDATE],
			sent_feedback: None,
		},
	};

	pending_response
		.send_outgoing_response(response)
		.map_err(|_| JfyiError::SendResponse(peer))?;

	Ok(result)
}
