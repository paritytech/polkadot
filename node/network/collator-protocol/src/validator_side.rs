// Copyright 2020 Parity Technologies (UK) Ltd.
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

use std::{collections::{HashMap, HashSet}, sync::Arc, task::Poll};
use std::time::{Duration, Instant};
use always_assert::never;
use futures::{
	channel::oneshot, future::{BoxFuture, Either, Fuse, FusedFuture}, FutureExt, StreamExt,
};
use futures_timer::Delay;

use polkadot_node_network_protocol::{
	request_response as req_res, v1 as protocol_v1,
	peer_set::PeerSet,
	request_response::{
		request::{Recipient, RequestError},
		v1::{CollationFetchingRequest, CollationFetchingResponse},
		OutgoingRequest, Requests,
	},
	OurView, PeerId, UnifiedReputationChange as Rep, View,
};
use polkadot_node_primitives::{SignedFullStatement, Statement, PoV};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_primitives::v1::{CandidateReceipt, CollatorId, Hash, Id as ParaId};
use polkadot_subsystem::{
	jaeger,
	messages::{
		AllMessages, CandidateSelectionMessage, CollatorProtocolMessage, IfDisconnected,
		NetworkBridgeEvent, NetworkBridgeMessage,
	},
	FromOverseer, OverseerSignal, PerLeafSpan, SubsystemContext,
};

use super::{modify_reputation, Result, LOG_TARGET};

const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("An unexpected message");
/// Message could not be decoded properly.
const COST_CORRUPTED_MESSAGE: Rep = Rep::CostMinor("Message was corrupt");
/// Network errors that originated at the remote host should have same cost as timeout.
const COST_NETWORK_ERROR: Rep = Rep::CostMinor("Some network error");
const COST_REQUEST_TIMED_OUT: Rep = Rep::CostMinor("A collation request has timed out");
const COST_INVALID_SIGNATURE: Rep = Rep::Malicious("Invalid network message signature");
const COST_REPORT_BAD: Rep = Rep::Malicious("A collator was reported by another subsystem");
const BENEFIT_NOTIFY_GOOD: Rep = Rep::BenefitMinor("A collator was noted good by another subsystem");

// How often to check all peers with activity.
#[cfg(not(test))]
const ACTIVITY_POLL: Duration = Duration::from_secs(1);

#[cfg(test)]
const ACTIVITY_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_request(&self, succeeded: std::result::Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			match succeeded {
				Ok(()) => metrics.collation_requests.with_label_values(&["succeeded"]).inc(),
				Err(()) => metrics.collation_requests.with_label_values(&["failed"]).inc(),
			}
		}
	}

	/// Provide a timer for `process_msg` which observes on drop.
	fn time_process_msg(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_msg.start_timer())
	}

	/// Provide a timer for `handle_collation_request_result` which observes on drop.
	fn time_handle_collation_request_result(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_collation_request_result.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	collation_requests: prometheus::CounterVec<prometheus::U64>,
	process_msg: prometheus::Histogram,
	handle_collation_request_result: prometheus::Histogram,
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry)
		-> std::result::Result<Self, prometheus::PrometheusError>
	{
		let metrics = MetricsInner {
			collation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_collation_requests_total",
						"Number of collations requested from Collators.",
					),
					&["success"],
				)?,
				registry,
			)?,
			process_msg: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_collator_protocol_validator_process_msg",
						"Time spent within `collator_protocol_validator::process_msg`",
					)
				)?,
				registry,
			)?,
			handle_collation_request_result: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_collator_protocol_validator_handle_collation_request_result",
						"Time spent within `collator_protocol_validator::handle_collation_request_result`",
					)
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}

struct PerRequest {
	/// Responses from collator.
	from_collator: Fuse<BoxFuture<'static, req_res::OutgoingResult<CollationFetchingResponse>>>,
	/// Sender to forward to initial requester.
	to_requester: oneshot::Sender<(CandidateReceipt, PoV)>,
	/// A jaeger span corresponding to the lifetime of the request.
	span: Option<jaeger::Span>,
}

struct PeerData {
	view: View,
	last_active: Instant,
}

impl PeerData {
	fn new(view: View) -> Self {
		PeerData {
			view,
			last_active: Instant::now(),
		}
	}

	fn note_active(&mut self) {
		self.last_active = Instant::now();
	}

	fn active_since(&self, instant: Instant) -> bool {
		self.last_active >= instant
	}
}

impl Default for PeerData {
	fn default() -> Self {
		PeerData::new(Default::default())
	}
}

/// All state relevant for the validator side of the protocol lives here.
#[derive(Default)]
struct State {
	/// Our own view.
	view: OurView,

	/// Track all active collators and their data.
	peer_data: HashMap<PeerId, PeerData>,

	/// Peers that have declared themselves as collators.
	known_collators: HashMap<PeerId, CollatorId>,

	/// Advertisements received from collators. We accept one advertisement
	/// per collator per source per relay-parent.
	advertisements: HashMap<PeerId, HashSet<(ParaId, Hash)>>,

	/// The collations we have requested by relay parent and para id.
	///
	/// For each relay parent and para id we may be connected to a number
	/// of collators each of those may have advertised a different collation.
	/// So we group such cases here.
	requested_collations: HashMap<(Hash, ParaId, PeerId), PerRequest>,

	/// Metrics.
	metrics: Metrics,

	/// Span per relay parent.
	span_per_relay_parent: HashMap<Hash, PerLeafSpan>,
}

/// Another subsystem has requested to fetch collations on a particular leaf for some para.
#[tracing::instrument(level = "trace", skip(ctx, state, tx), fields(subsystem = LOG_TARGET))]
async fn fetch_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	collator_id: CollatorId,
	para_id: ParaId,
	tx: oneshot::Sender<(CandidateReceipt, PoV)>
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	let relevant_advertiser = state.advertisements.iter().find_map(|(k, v)| {
		if v.contains(&(para_id, relay_parent)) && state.known_collators.get(k) == Some(&collator_id) {
			Some(k.clone())
		} else {
			None
		}
	});

	// Request the collation.
	// Assume it is `request_collation`'s job to check and ignore duplicate requests.
	if let Some(relevant_advertiser) = relevant_advertiser {
		request_collation(ctx, state, relay_parent, para_id, relevant_advertiser, tx).await;
	}
}

/// Report a collator for some malicious actions.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn report_collator<Context>(
	ctx: &mut Context,
	state: &mut State,
	id: CollatorId,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	// Since we have a one way map of PeerId -> CollatorId we have to
	// iterate here. Since a huge amount of peers is not expected this
	// is a tolerable thing to do.
	for (k, _) in state.known_collators.iter().filter(|d| *d.1 == id) {
		modify_reputation(ctx, k.clone(), COST_REPORT_BAD).await;
	}
}

/// Some other subsystem has reported a collator as a good one, bump reputation.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn note_good_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	id: CollatorId,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	for (peer_id, _) in state.known_collators.iter().filter(|d| *d.1 == id) {
		modify_reputation(ctx, peer_id.clone(), BENEFIT_NOTIFY_GOOD).await;
	}
}

/// Notify a collator that its collation got seconded.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn notify_collation_seconded(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	state: &mut State,
	id: CollatorId,
	statement: SignedFullStatement,
) {
	if !matches!(statement.payload(), Statement::Seconded(_)) {
		tracing::error!(
			target: LOG_TARGET,
			statement = ?statement,
			"Notify collation seconded called with a wrong statement.",
		);
		return;
	}

	let peer_ids = state.known_collators.iter()
		.filter_map(|(p, c)| if *c == id { Some(p.clone()) } else { None })
		.collect::<Vec<_>>();

	if !peer_ids.is_empty() {
		let wire_message = protocol_v1::CollatorProtocolMessage::CollationSeconded(statement);

		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendCollationMessage(
				peer_ids,
				protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
			)
		)).await;
	}
}

/// A peer's view has changed. A number of things should be done:
///  - Ongoing collation requests have to be cancelled.
///  - Advertisements by this peer that are no longer relevant have to be removed.
#[tracing::instrument(level = "trace", skip(state), fields(subsystem = LOG_TARGET))]
async fn handle_peer_view_change(
	state: &mut State,
	peer_id: PeerId,
	view: View,
) -> Result<()> {
	let current = state.peer_data.entry(peer_id.clone()).or_default();

	let removed: Vec<_> = current.view.difference(&view).cloned().collect();

	current.view = view;

	if let Some(advertisements) = state.advertisements.get_mut(&peer_id) {
		advertisements.retain(|(_, relay_parent)| !removed.contains(relay_parent));
	}

	for removed in removed.into_iter() {
		state.requested_collations.retain(|k, _| k.0 != removed || k.2 != peer_id);
	}

	Ok(())
}

/// Request a collation from the network.
/// This function will
///  - Check for duplicate requests.
///  - Check if the requested collation is in our view.
///  - Update PerRequest records with the `result` field if necessary.
/// And as such invocations of this function may rely on that.
#[tracing::instrument(level = "trace", skip(ctx, state, result), fields(subsystem = LOG_TARGET))]
async fn request_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	result: oneshot::Sender<(CandidateReceipt, PoV)>,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	if !state.view.contains(&relay_parent) {
		tracing::debug!(
			target: LOG_TARGET,
			peer_id = %peer_id,
			para_id = %para_id,
			relay_parent = %relay_parent,
			"collation is no longer in view",
		);
		return;
	}

	if state.requested_collations.contains_key(&(relay_parent, para_id.clone(), peer_id.clone())) {
		tracing::warn!(
			target: LOG_TARGET,
			peer_id = %peer_id,
			%para_id,
			?relay_parent,
			"collation has already been requested",
		);
		return;
	}

	let (full_request, response_recv) =
		OutgoingRequest::new(Recipient::Peer(peer_id), CollationFetchingRequest {
			relay_parent,
			para_id,
		});
	let requests = Requests::CollationFetching(full_request);

	let per_request = PerRequest {
		from_collator: response_recv.boxed().fuse(),
		to_requester: result,
		span: state.span_per_relay_parent.get(&relay_parent).map(|s| {
			s.child("collation-request")
				.with_para_id(para_id)
		}),

	};

	state.requested_collations.insert((relay_parent, para_id.clone(), peer_id.clone()), per_request);

	tracing::debug!(
		target: LOG_TARGET,
		peer_id = %peer_id,
		%para_id,
		?relay_parent,
		"Requesting collation",
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendRequests(vec![requests], IfDisconnected::ImmediateError))
	).await;
}

/// Notify `CandidateSelectionSubsystem` that a collation has been advertised.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn notify_candidate_selection<Context>(
	ctx: &mut Context,
	collator: CollatorId,
	relay_parent: Hash,
	para_id: ParaId,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	ctx.send_message(AllMessages::CandidateSelection(
		CandidateSelectionMessage::Collation(
			relay_parent,
			para_id,
			collator,
		)
	)).await;
}

/// Networking message has been received.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use protocol_v1::CollatorProtocolMessage::*;
	use sp_runtime::traits::AppVerify;

	if let Some(d) = state.peer_data.get_mut(&origin) {
		d.note_active();
	}

	match msg {
		Declare(id, signature) => {
			if !signature.verify(&*protocol_v1::declare_signature_payload(&origin), &id) {
				modify_reputation(ctx, origin, COST_INVALID_SIGNATURE).await;
				return;
			}

			tracing::debug!(
				target: LOG_TARGET,
				peer_id = ?origin,
				"Declared as collator",
			);

			if state.known_collators.insert(origin.clone(), id).is_some() {
				modify_reputation(ctx, origin.clone(), COST_UNEXPECTED_MESSAGE).await;
			}
		}
		AdvertiseCollation(relay_parent, para_id) => {
			let _span = state.span_per_relay_parent.get(&relay_parent).map(|s| s.child("advertise-collation"));

			if !state.view.contains(&relay_parent) {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					%para_id,
					?relay_parent,
					"Advertise collation out of view",
				);

				modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				return;
			}

			if !state.advertisements.entry(origin.clone()).or_default().insert((para_id, relay_parent)) {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					%para_id,
					?relay_parent,
					"Multiple collations for same relay-parent advertised",
				);

				modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				return;
			}

			if let Some(collator) = state.known_collators.get(&origin) {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					%para_id,
					?relay_parent,
					"Received advertise collation",
				);

				notify_candidate_selection(ctx, collator.clone(), relay_parent, para_id).await;
			} else {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					%para_id,
					?relay_parent,
					"Advertise collation received from an unknown collator",
				);
			}
		}
		CollationSeconded(_) => {
			tracing::warn!(
				target: LOG_TARGET,
				peer_id = ?origin,
				"Unexpected `CollationSeconded` message, decreasing reputation",
			);
			modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
		}
	}
}

/// A leaf has become inactive so we want to
///   - Cancel all ongoing collation requests that are on top of that leaf.
///   - Remove all stored collations relevant to that leaf.
#[tracing::instrument(level = "trace", skip(state), fields(subsystem = LOG_TARGET))]
async fn remove_relay_parent(
	state: &mut State,
	relay_parent: Hash,
) -> Result<()> {
	state.requested_collations.retain(|k, _| {
		k.0 != relay_parent
	});
	Ok(())
}

/// Our view has changed.
#[tracing::instrument(level = "trace", skip(state), fields(subsystem = LOG_TARGET))]
async fn handle_our_view_change(
	state: &mut State,
	view: OurView,
) -> Result<()> {
	let old_view = std::mem::replace(&mut state.view, view);

	let added: HashMap<Hash, Arc<jaeger::Span>> = state.view
		.span_per_head()
		.iter()
		.filter(|v| !old_view.contains(&v.0))
		.map(|v| (v.0.clone(), v.1.clone()))
		.collect();
	added.into_iter().for_each(|(h, s)| {
		state.span_per_relay_parent.insert(h, PerLeafSpan::new(s, "validator-side"));
	});

	let removed = old_view
		.difference(&state.view)
		.cloned()
		.collect::<Vec<_>>();

	for removed in removed.into_iter() {
		remove_relay_parent(state, removed).await?;
		state.span_per_relay_parent.remove(&removed);
	}

	Ok(())
}

/// Bridge event switch.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut State,
	bridge_message: NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(peer_id, _role) => {
			state.peer_data.entry(peer_id).or_default();
		},
		PeerDisconnected(peer_id) => {
			state.known_collators.remove(&peer_id);
			state.peer_data.remove(&peer_id);
		},
		PeerViewChange(peer_id, view) => {
			handle_peer_view_change(state, peer_id, view).await?;
		},
		OurViewChange(view) => {
			handle_our_view_change(state, view).await?;
		},
		PeerMessage(remote, msg) => {
			process_incoming_peer_message(ctx, state, remote, msg).await;
		}
	}

	Ok(())
}

/// The main message receiver switch.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn process_msg<Context>(
	ctx: &mut Context,
	msg: CollatorProtocolMessage,
	state: &mut State,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use CollatorProtocolMessage::*;

	let _timer = state.metrics.time_process_msg();

	match msg {
		CollateOn(id) => {
			tracing::warn!(
				target: LOG_TARGET,
				para_id = %id,
				"CollateOn message is not expected on the validator side of the protocol",
			);
		}
		DistributeCollation(_, _, _) => {
			tracing::warn!(
				target: LOG_TARGET,
				"DistributeCollation message is not expected on the validator side of the protocol",
			);
		}
		FetchCollation(relay_parent, collator_id, para_id, tx) => {
			let _span = state.span_per_relay_parent.get(&relay_parent).map(|s| s.child("fetch-collation"));
			fetch_collation(ctx, state, relay_parent, collator_id, para_id, tx).await;
		}
		ReportCollator(id) => {
			report_collator(ctx, state, id).await;
		}
		NoteGoodCollation(id) => {
			note_good_collation(ctx, state, id).await;
		}
		NotifyCollationSeconded(id, statement) => {
			notify_collation_seconded(ctx, state, id, statement).await;
		}
		NetworkBridgeUpdateV1(event) => {
			if let Err(e) = handle_network_msg(
				ctx,
				state,
				event,
			).await {
				tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to handle incoming network message",
				);
			}
		}
		CollationFetchingRequest(_) => {
			tracing::warn!(
				target: LOG_TARGET,
				"CollationFetchingRequest message is not expected on the validator side of the protocol",
			);
		}
	}
}

// wait until next inactivity check. returns the instant for the following check.
async fn wait_until_next_check(last_poll: Instant) -> Instant {
	let now = Instant::now();
	let next_poll = last_poll + ACTIVITY_POLL;

	if next_poll > now {
		Delay::new(next_poll - now).await
	}

	Instant::now()
}

/// The main run loop.
#[tracing::instrument(skip(ctx, metrics), fields(subsystem = LOG_TARGET))]
pub(crate) async fn run<Context>(
	mut ctx: Context,
	eviction_policy: crate::CollatorEvictionPolicy,
	metrics: Metrics,
) -> Result<()>
	where Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State {
		metrics,
		..Default::default()
	};

	let next_inactivity_stream = futures::stream::unfold(
		Instant::now() + ACTIVITY_POLL,
		|next_check| async move { Some(((), wait_until_next_check(next_check).await)) }
	).fuse();

	futures::pin_mut!(next_inactivity_stream);

	loop {
		let res = {
			let s = futures::future::select(ctx.recv().fuse(), next_inactivity_stream.next().fuse());

			if let Poll::Ready(res) = futures::poll!(s) {
				Some(match res {
					Either::Left((msg, _)) => Either::Left(msg?),
					Either::Right((_, _)) => Either::Right(()),
				})
			} else {
				None
			}
		};

		match res {
			Some(Either::Left(msg)) => {
				tracing::trace!(target: LOG_TARGET, msg = ?msg, "received a message");

				match msg {
					Communication { msg } => process_msg(&mut ctx, msg, &mut state).await,
					Signal(BlockFinalized(..)) => {}
					Signal(ActiveLeaves(_)) => {}
					Signal(Conclude) => { break }
				}

				continue
			}
			Some(Either::Right(())) => {
				disconnect_inactive_peers(&mut ctx, eviction_policy, &state.peer_data).await;
				continue
			}
			None => {}
		}

		let mut retained_requested = HashSet::new();
		for ((hash, para_id, peer_id), per_req) in state.requested_collations.iter_mut() {
			// Despite the await, this won't block on the response itself.
			let finished = poll_collation_response(
				&mut ctx, &state.metrics, &state.span_per_relay_parent,
				hash, para_id, peer_id, per_req
			).await;
			if !finished {
				retained_requested.insert((*hash, *para_id, *peer_id));
			}
		}
		state.requested_collations.retain(|k, _| retained_requested.contains(k));
		futures::pending!();
	}
	Ok(())
}

// This issues `NetworkBridge` notifications to disconnect from all inactive peers at the
// earliest possible point. This does not yet clean up any metadata, as that will be done upon
// receipt of the `PeerDisconnected` event.
async fn disconnect_inactive_peers(
	ctx: &mut impl SubsystemContext,
	eviction_policy: crate::CollatorEvictionPolicy,
	peers: &HashMap<PeerId, PeerData>,
) {
	let cutoff = match Instant::now().checked_sub(eviction_policy.0) {
		None => return,
		Some(i) => i,
	};

	for (peer, peer_data) in peers {
		if !peer_data.active_since(cutoff) {
			ctx.send_message(
				NetworkBridgeMessage::DisconnectPeer(peer.clone(), PeerSet::Collation).into()
			).await;
		}
	}
}

/// Poll collation response, return immediately if there is none.
///
/// Ready responses are handled, by logging and decreasing peer's reputation on error and by
/// forwarding proper responses to the requester.
///
/// Returns: `true` if `from_collator` future was ready.
async fn poll_collation_response<Context>(
	ctx: &mut Context,
	metrics: &Metrics,
	spans: &HashMap<Hash, PerLeafSpan>,
	hash: &Hash,
	para_id: &ParaId,
	peer_id: &PeerId,
	per_req: &mut PerRequest
)
-> bool
where
	Context: SubsystemContext
{
	if never!(per_req.from_collator.is_terminated()) {
		tracing::error!(
			target: LOG_TARGET,
			"We remove pending responses once received, this should not happen."
		);
		return true
	}

	if let Poll::Ready(response) = futures::poll!(&mut per_req.from_collator) {
		let _span = spans.get(&hash)
				.map(|s| s.child("received-collation"));
		let _timer = metrics.time_handle_collation_request_result();

		let mut metrics_result = Err(());
		let mut success = "false";

		match response {
			Err(RequestError::InvalidResponse(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					hash = ?hash,
					para_id = ?para_id,
					peer_id = ?peer_id,
					err = ?err,
					"Collator provided response that could not be decoded"
				);
				modify_reputation(ctx, *peer_id, COST_CORRUPTED_MESSAGE).await;
			}
			Err(RequestError::NetworkError(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					hash = ?hash,
					para_id = ?para_id,
					peer_id = ?peer_id,
					err = ?err,
					"Fetching collation failed due to network error"
				);
				// A minor decrease in reputation for any network failure seems
				// sensbile. In theory this could be exploited, by DoSing this node,
				// which would result in reduced reputation for proper nodes, but the
				// same can happen for penalities on timeouts, which we also have.
				modify_reputation(ctx, *peer_id, COST_NETWORK_ERROR).await;
			}
			Err(RequestError::Canceled(_)) => {
				tracing::warn!(
					target: LOG_TARGET,
					hash = ?hash,
					para_id = ?para_id,
					peer_id = ?peer_id,
					"Request timed out"
				);
				// A minor decrease in reputation for any network failure seems
				// sensbile. In theory this could be exploited, by DoSing this node,
				// which would result in reduced reputation for proper nodes, but the
				// same can happen for penalities on timeouts, which we also have.
				modify_reputation(ctx, *peer_id, COST_REQUEST_TIMED_OUT).await;
			}
			Ok(CollationFetchingResponse::Collation(receipt, compressed_pov)) => {
				match compressed_pov.decompress() {
					Ok(pov) => {
						tracing::debug!(
							target: LOG_TARGET,
							para_id = %para_id,
							hash = ?hash,
							candidate_hash = ?receipt.hash(),
							"Received collation",
						);

						// Actual sending:
						let _span = jaeger::Span::new(&pov, "received-collation");
						let (mut tx, _) = oneshot::channel();
						std::mem::swap(&mut tx, &mut (per_req.to_requester));
						let result = tx.send((receipt, pov));

						if let Err(_) = result  {
							tracing::warn!(
								target: LOG_TARGET,
								hash = ?hash,
								para_id = ?para_id,
								peer_id = ?peer_id,
								"Sending response back to requester failed (receiving side closed)"
							);
						} else {
							metrics_result = Ok(());
							success = "true";
						}

					}
					Err(error) => {
						tracing::warn!(
							target: LOG_TARGET,
							hash = ?hash,
							para_id = ?para_id,
							peer_id = ?peer_id,
							?error,
							"Failed to extract PoV",
						);
						modify_reputation(ctx, *peer_id, COST_CORRUPTED_MESSAGE).await;
					}
				};
			}
		};
		metrics.on_request(metrics_result);
		per_req.span.as_mut().map(|s| s.add_string_tag("success", success));
		true
	} else {
		false
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{iter, time::Duration};
	use futures::{executor, future, Future};
	use polkadot_node_subsystem_util::TimeoutExt;
	use sp_core::{crypto::Pair, Encode};
	use assert_matches::assert_matches;

	use polkadot_primitives::v1::CollatorPair;
	use polkadot_node_primitives::{BlockData, CompressedPoV};
	use polkadot_subsystem_testhelpers as test_helpers;
	use polkadot_node_network_protocol::{our_view, ObservedRole,
		request_response::Requests
	};

	const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(50);

	#[derive(Clone)]
	struct TestState {
		chain_ids: Vec<ParaId>,
		relay_parent: Hash,
		collators: Vec<CollatorPair>,
	}

	impl Default for TestState {
		fn default() -> Self {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);

			let chain_ids = vec![chain_a, chain_b];
			let relay_parent = Hash::repeat_byte(0x05);
			let collators = iter::repeat(())
				.map(|_| CollatorPair::generate().0)
				.take(4)
				.collect();

			Self {
				chain_ids,
				relay_parent,
				collators,
			}
		}
	}

	struct TestHarness {
		virtual_overseer: test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	}

	fn test_harness<T: Future<Output = ()>>(test: impl FnOnce(TestHarness) -> T) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				Some("polkadot_collator_protocol"),
				log::LevelFilter::Trace,
			)
			.filter(
				Some(LOG_TARGET),
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = run(
			context,
			crate::CollatorEvictionPolicy(ACTIVITY_TIMEOUT),
			Metrics::default(),
		);

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(200);

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		msg: CollatorProtocolMessage,
	) {
		tracing::trace!("Sending message:\n{:?}", &msg);
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

		tracing::trace!("Received message:\n{:?}", &msg);

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		timeout: Duration,
	) -> Option<AllMessages> {
		tracing::trace!("Waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	// As we receive a relevant advertisement act on it and issue a collation request.
	#[test]
	fn act_on_advertisement() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;
			tracing::trace!("activating");

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
				)
			).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							pair.public(),
							pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				assert_eq!(collator, pair.public());
			});
		});
	}

	// Test that other subsystems may modify collators' reputations.
	#[test]
	fn collator_reporting_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
				)
			).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
							test_state.collators[0].sign(&protocol_v1::declare_signature_payload(&peer_b)),
						),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[1].public(),
							test_state.collators[1].sign(&protocol_v1::declare_signature_payload(&peer_c)),
						),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::ReportCollator(test_state.collators[0].public()),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep),
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_REPORT_BAD);
				}
			);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NoteGoodCollation(test_state.collators[1].public()),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep),
				) => {
					assert_eq!(peer, peer_c);
					assert_eq!(rep, BENEFIT_NOTIFY_GOOD);
				}
			);
		});
	}

	// Test that we verify the signatures on `Declare` and `AdvertiseCollation` messages.
	#[test]
	fn collator_authentication_verification_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let peer_b = PeerId::random();

			// the peer sends a declare message but sign the wrong payload
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[0].public(),
						test_state.collators[0].sign(&[42]),
					),
				)),
			)
			.await;

			// it should be reported for sending a message with an invalid signature
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep),
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_INVALID_SIGNATURE);
				}
			);
		});
	}

	// A test scenario that takes the following steps
	//  - Two collators connect, declare themselves and advertise a collation relevant to
	//    our view.
	//  - This results subsystem acting upon these advertisements and issuing two messages to
	//    the CandidateBacking subsystem.
	//  - CandidateBacking requests both of the collations.
	//  - Collation protocol requests these collations.
	//  - The collations are sent to it.
	//  - Collations are fetched correctly.
	#[test]
	fn fetch_collations_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
				),
			).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
							test_state.collators[0].sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[1].public(),
							test_state.collators[1].sign(&protocol_v1::declare_signature_payload(&peer_c)),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				assert_eq!(collator, test_state.collators[0].public());
			});

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				assert_eq!(collator, test_state.collators[1].public());
			});

			let (tx_0, rx_0) = oneshot::channel();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::FetchCollation(
					test_state.relay_parent,
					test_state.collators[0].public(),
					test_state.chain_ids[0],
					tx_0,
				)
			).await;

			let (tx_1, rx_1) = oneshot::channel();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::FetchCollation(
					test_state.relay_parent,
					test_state.collators[1].public(),
					test_state.chain_ids[0],
					tx_1,
				)
			).await;

			let response_channel = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
			) => {
				let req = reqs.into_iter().next()
					.expect("There should be exactly one request");
				match req {
					Requests::CollationFetching(req) => {
						let payload = req.payload;
						assert_eq!(payload.relay_parent, test_state.relay_parent);
						assert_eq!(payload.para_id, test_state.chain_ids[0]);
						req.pending_response
					}
					_ => panic!("Unexpected request"),
				}
			});

			let mut candidate_a = CandidateReceipt::default();
			candidate_a.descriptor.para_id = test_state.chain_ids[0];
			candidate_a.descriptor.relay_parent = test_state.relay_parent;
			response_channel.send(Ok(
				CollationFetchingResponse::Collation(
					candidate_a.clone(),
					CompressedPoV::compress(&PoV {
						block_data: BlockData(vec![]),
					}).unwrap(),
				).encode()
			)).expect("Sending response should succeed");

			let response_channel = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
			) => {
				let req = reqs.into_iter().next()
					.expect("There should be exactly one request");
				match req {
					Requests::CollationFetching(req) => {
						let payload = req.payload;
						assert_eq!(payload.relay_parent, test_state.relay_parent);
						assert_eq!(payload.para_id, test_state.chain_ids[0]);
						req.pending_response
					}
					_ => panic!("Unexpected request"),
				}
			});

			let mut candidate_b = CandidateReceipt::default();
			candidate_b.descriptor.para_id = test_state.chain_ids[0];
			candidate_b.descriptor.relay_parent = test_state.relay_parent;

			response_channel.send(Ok(
				CollationFetchingResponse::Collation(
					candidate_b.clone(),
					CompressedPoV::compress(&PoV {
						block_data: BlockData(vec![1, 2, 3]),
					}).unwrap(),
				).encode()
			)).expect("Sending response should succeed");

			let collation_0 = rx_0.await.unwrap();
			let collation_1 = rx_1.await.unwrap();

			assert_eq!(collation_0.0, candidate_a);
			assert_eq!(collation_1.0, candidate_b);
		});
	}

	#[test]
	fn inactive_disconnected() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;
			tracing::trace!("activating");

			let hash_a = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_a])
				)
			).await;


			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							pair.public(),
							pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				assert_eq!(collator, pair.public());
			});

			Delay::new(ACTIVITY_TIMEOUT * 2).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					peer,
					peer_set,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(peer_set, PeerSet::Collation);
				}
			)
		});
	}

	#[test]
	fn activity_extends_life() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;
			tracing::trace!("activating");

			let hash_a = test_state.relay_parent;
			let hash_b = Hash::repeat_byte(1);
			let hash_c = Hash::repeat_byte(2);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_a, hash_b, hash_c])
				)
			).await;


			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
					)
				)
			).await;

			Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							pair.public(),
							pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				assert_eq!(collator, pair.public());
			});

			Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							hash_b,
							test_state.chain_ids[1],
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator,
				)
			) => {
				assert_eq!(relay_parent, hash_b);
				assert_eq!(para_id, test_state.chain_ids[1]);
				assert_eq!(collator, pair.public());
			});

			Delay::new(ACTIVITY_TIMEOUT * 3 / 2).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					peer,
					peer_set,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(peer_set, PeerSet::Collation);
				}
			)
		});
	}
}
