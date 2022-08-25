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
	collections::{HashMap, HashSet, VecDeque},
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

use futures::{
	channel::oneshot,
	future::{poll_fn, BoxFuture, Fuse},
	pin_mut, select_biased,
	stream::{FusedStream, FuturesUnordered, StreamExt},
	Future, FutureExt, Stream,
};
use futures_timer::Delay;
use lru::LruCache;

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
use polkadot_primitives::v2::AuthorityDiscoveryId;

use crate::{
	metrics::{FAILED, SUCCEEDED},
	Metrics, LOG_TARGET, RECEIVE_RATE_LIMIT,
};

mod error;

/// Queues for incoming requests by peers.
mod peer_queues;

/// Batch imports together.
mod batch;

use self::{
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
/// See `MIN_KEEP_BATCH_ALIVE_VOTES` above.
pub const BATCH_COLLECTING_INTERVAL: Duration = Duration::from_millis(500);

/// State for handling incoming `DisputeRequest` messages.
///
/// This is supposed to run as its own task in order to easily impose back pressure on the incoming
/// request channel and at the same time to drop flood messages as fast as possible.
pub struct DisputesReceiver<Sender, AD> {
	/// Access to session information.
	runtime: RuntimeInfo,

	/// Subsystem sender for communication with other subsystems.
	sender: Sender,

	/// Channel to retrieve incoming requests from.
	receiver: IncomingRequestReceiver<DisputeRequest>,

	/// Rate limiting queue for each peer (only authorities).
	peer_queues: PeerQueues,

	/// Delay timer for establishing the rate limit.
	rate_limit: Fuse<Delay>,

	/// Authority discovery service:
	authority_discovery: AD,

	/// Imports currently being processed.
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
}

impl MuxedMessage {
	async fn receive(
		pending_imports: &mut PendingImports,
		pending_requests: &mut IncomingRequestReceiver<DisputeRequest>,
	) -> Result<MuxedMessage> {
		poll_fn(|ctx| {
			// In case of Ready(None), we want to wait for pending requests:
			if let Poll::Ready(Some(v)) = pending_imports.poll_next_unpin(ctx) {
				return Poll::Ready(Ok(Self::ConfirmedImport(v)))
			}

			let next_req = pending_requests.recv(|| vec![COST_INVALID_REQUEST]);
			pin_mut!(next_req);
			if let Poll::Ready(r) = next_req.poll(ctx) {
				return match r {
					Err(e) => Poll::Ready(Err(incoming::Error::from(e).into())),
					Ok(v) => Poll::Ready(Ok(Self::NewRequest(v))),
				}
			}
			Poll::Pending
		})
		.await
	}
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
			rate_limit: Delay::new(RECEIVE_RATE_LIMIT).fuse(),
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

	/// Actual work happening here.
	async fn run_inner(&mut self) -> Result<()> {
		let msg = if self.peer_queues.is_empty() {
			// No point to wake on timeout:
			Some(MuxedMessage::receive(&mut self.pending_imports, &mut self.receiver).await?)
		} else {
			self.wait_for_message_or_timeout().await?
		};

		if let Some(msg) = msg {
			let incoming = match msg {
				// We need to clean up futures, to make sure responses are sent:
				MuxedMessage::ConfirmedImport(m_bad) => {
					self.ban_bad_peer(m_bad)?;
					return Ok(())
				},
				MuxedMessage::NewRequest(req) => req,
			};

			self.metrics.on_received_request();
			self.dispatch_to_queues(incoming).await?;
			// Wait for more messages:
			return Ok(())
		}

		// Let's actually process messages, that made it through the rate limit:
		//
		// Batch:
		// - Collect votes - get rid of duplicates.
		// - Keep track of import rate.
		// - Flush if import rate is not matched
		// Wait for a free slot:
        //
        // struct Batch {
        //  
        // }
		if self.pending_imports.len() >= MAX_PARALLEL_IMPORTS as usize {
			// Wait for one to finish:
			let r = self.pending_imports.next().await;
			self.ban_bad_peer(r.expect("pending_imports.len() is greater 0. qed."))?;
		}

		// All good - initiate import.
		self.start_import(incoming).await
	}

	/// Wait for a message or the `rate_limit` timeout to hit (if there is one).
	///
	/// In case a message got received `rate_limit` will be populated by this function. This way we
	/// only wake on timeouts if there are actually any messages to process.
	///
	/// In case of timeout we return Ok(None).
	async fn wait_for_message_or_timeout(&mut self) -> Result<Option<MuxedMessage>> {
		// We already have messages to process - rate limiting activated:
		let rcv_msg = MuxedMessage::receive(&mut self.pending_imports, &mut self.receiver).fuse();
		pin_mut!(rcv_msg);
		let mut timeout = Pin::new(&mut self.rate_limit);
		let result = select_biased!(
			() = timeout => None,
			msg = rcv_msg => Some(msg?),
		);
		if result.is_none() {
			// Timeout hit - we need a new Delay (started immediately so the following processing
			// does not further decrease allowed rate (assuming processing takes less than
			// `RECEIVE_RATE_LIMIT`):
			self.rate_limit = Delay::new(RECEIVE_RATE_LIMIT).fuse();
		}
		Ok(result)
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

	/// Start importing votes for the given request.
	async fn start_import(&mut self, incoming: IncomingRequest<DisputeRequest>) -> Result<()> {
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

		let (pending_confirmation, confirmation_rx) = oneshot::channel();
		self.sender
			.send_message(DisputeCoordinatorMessage::ImportStatements {
				candidate_receipt,
				session: valid_vote.0.session_index(),
				statements: vec![valid_vote, invalid_vote],
				pending_confirmation: Some(pending_confirmation),
			})
			.await;

		self.pending_imports.push(peer, confirmation_rx, pending_response);
		Ok(())
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
