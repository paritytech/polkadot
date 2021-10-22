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
};

use futures::{
	channel::oneshot,
	future::{poll_fn, BoxFuture},
	pin_mut,
	stream::{FusedStream, FuturesUnordered, StreamExt},
	Future, FutureExt, Stream,
};
use lru::LruCache;

use polkadot_node_network_protocol::{
	authority_discovery::AuthorityDiscovery,
	request_response::{
		incoming::{OutgoingResponse, OutgoingResponseSender},
		v1::{DisputeRequest, DisputeResponse},
		IncomingRequest, IncomingRequestReceiver,
	},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::DISPUTE_WINDOW;
use polkadot_node_subsystem_util::{runtime, runtime::RuntimeInfo};
use polkadot_subsystem::{
	messages::{AllMessages, DisputeCoordinatorMessage, ImportStatementsResult},
	SubsystemSender,
};

use crate::{
	metrics::{FAILED, SUCCEEDED},
	Metrics, LOG_TARGET,
};

mod error;
use self::error::{log_error, NonFatal, NonFatalResult, Result};

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Received message could not be decoded.");
const COST_INVALID_SIGNATURE: Rep = Rep::Malicious("Signatures were invalid.");
const COST_INVALID_CANDIDATE: Rep = Rep::Malicious("Reported candidate was not available.");
const COST_NOT_A_VALIDATOR: Rep = Rep::CostMajor("Reporting peer was not a validator.");

/// How many statement imports we want to issue in parallel:
pub const MAX_PARALLEL_IMPORTS: usize = 10;

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

	/// Authority discovery service:
	authority_discovery: AD,

	/// Imports currently being processed.
	pending_imports: PendingImports,

	/// We keep record of the last banned peers.
	///
	/// This is needed because once we ban a peer, we will very likely still have pending requests
	/// in the incoming channel - we should not waste time recovering availability for those, as we
	/// already know the peer is malicious.
	banned_peers: LruCache<PeerId, ()>,

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
	ConfirmedImport(NonFatalResult<(PeerId, ImportStatementsResult)>),

	/// A new request has arrived and should be handled.
	NewRequest(IncomingRequest<DisputeRequest>),
}

impl MuxedMessage {
	async fn receive(
		pending_imports: &mut PendingImports,
		pending_requests: &mut IncomingRequestReceiver<DisputeRequest>,
	) -> Result<MuxedMessage> {
		poll_fn(|ctx| {
			let next_req = pending_requests.recv(|| vec![COST_INVALID_REQUEST]);
			pin_mut!(next_req);
			if let Poll::Ready(r) = next_req.poll(ctx) {
				return match r {
					Err(e) => Poll::Ready(Err(e.into())),
					Ok(v) => Poll::Ready(Ok(Self::NewRequest(v))),
				}
			}
			// In case of Ready(None) return `Pending` below - we want to wait for the next request
			// in that case.
			if let Poll::Ready(Some(v)) = pending_imports.poll_next_unpin(ctx) {
				return Poll::Ready(Ok(Self::ConfirmedImport(v)))
			}
			Poll::Pending
		})
		.await
	}
}

impl<Sender: SubsystemSender, AD> DisputesReceiver<Sender, AD>
where
	AD: AuthorityDiscovery,
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
			authority_discovery,
			pending_imports: PendingImports::new(),
			// Size of MAX_PARALLEL_IMPORTS ensures we are going to immediately get rid of any
			// malicious requests still pending in the incoming queue.
			banned_peers: LruCache::new(MAX_PARALLEL_IMPORTS),
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
					tracing::debug!(
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
		let msg = MuxedMessage::receive(&mut self.pending_imports, &mut self.receiver).await?;

		let incoming = match msg {
			// We need to clean up futures, to make sure responses are sent:
			MuxedMessage::ConfirmedImport(m_bad) => {
				self.ban_bad_peer(m_bad)?;
				return Ok(())
			},
			MuxedMessage::NewRequest(req) => req,
		};

		self.metrics.on_received_request();

		let peer = incoming.peer;

		// Only accept messages from validators:
		if self.authority_discovery.get_authority_id_by_peer_id(peer).await.is_none() {
			incoming
				.send_outgoing_response(OutgoingResponse {
					result: Err(()),
					reputation_changes: vec![COST_NOT_A_VALIDATOR],
					sent_feedback: None,
				})
				.map_err(|_| NonFatal::SendResponse(peer))?;

			return Err(NonFatal::NotAValidator(peer).into())
		}

		// Immediately drop requests from peers that already have requests in flight or have
		// been banned recently (flood protection):
		if self.pending_imports.peer_is_pending(&peer) || self.banned_peers.contains(&peer) {
			tracing::trace!(
				target: LOG_TARGET,
				?peer,
				"Dropping message from peer (banned/pending import)"
			);
			return Ok(())
		}

		// Wait for a free slot:
		if self.pending_imports.len() >= MAX_PARALLEL_IMPORTS as usize {
			// Wait for one to finish:
			let r = self.pending_imports.next().await;
			self.ban_bad_peer(r.expect("pending_imports.len() is greater 0. qed."))?;
		}

		// All good - initiate import.
		self.start_import(incoming).await
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
					.map_err(|_| NonFatal::SetPeerReputation(peer))?;

				return Err(From::from(NonFatal::InvalidSignature(peer)))
			},
			Ok(votes) => votes,
		};

		let (pending_confirmation, confirmation_rx) = oneshot::channel();
		let candidate_hash = candidate_receipt.hash();
		self.sender
			.send_message(AllMessages::DisputeCoordinator(
				DisputeCoordinatorMessage::ImportStatements {
					candidate_hash,
					candidate_receipt,
					session: valid_vote.0.session_index(),
					statements: vec![valid_vote, invalid_vote],
					pending_confirmation,
				},
			))
			.await;

		self.pending_imports.push(peer, confirmation_rx, pending_response);
		Ok(())
	}

	/// Await an import and ban any misbehaving peers.
	///
	/// In addition we report import metrics.
	fn ban_bad_peer(
		&mut self,
		result: NonFatalResult<(PeerId, ImportStatementsResult)>,
	) -> NonFatalResult<()> {
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
	futures: FuturesUnordered<BoxFuture<'static, (PeerId, NonFatalResult<ImportStatementsResult>)>>,
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
	type Item = NonFatalResult<(PeerId, ImportStatementsResult)>;
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
) -> NonFatalResult<ImportStatementsResult> {
	let result = handled.await.map_err(|_| NonFatal::ImportCanceled(peer))?;

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
		.map_err(|_| NonFatal::SendResponse(peer))?;

	Ok(result)
}
