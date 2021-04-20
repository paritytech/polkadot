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
use std::collections::hash_map::Entry;
use std::time::{Duration, Instant};
use always_assert::never;
use futures::{
	channel::oneshot, future::{BoxFuture, Either, Fuse, FusedFuture}, FutureExt, StreamExt,
};
use futures_timer::Delay;

use sp_keystore::SyncCryptoStorePtr;

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
	FromOverseer, OverseerSignal, PerLeafSpan, SubsystemContext, SubsystemSender,
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
const COST_WRONG_PARA: Rep = Rep::Malicious("A collator provided a collation for the wrong para");
const COST_UNNEEDED_COLLATOR: Rep = Rep::CostMinor("An unneeded collator connected");
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

	/// Note the current number of collator peers.
	fn note_collator_peer_count(&self, collator_peers: usize) {
		self.0.as_ref().map(|metrics| metrics.collator_peer_count.set(collator_peers as u64));
	}
}

#[derive(Clone)]
struct MetricsInner {
	collation_requests: prometheus::CounterVec<prometheus::U64>,
	process_msg: prometheus::Histogram,
	handle_collation_request_result: prometheus::Histogram,
	collator_peer_count: prometheus::Gauge<prometheus::U64>,
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
			collator_peer_count: prometheus::register(
				prometheus::Gauge::new(
					"parachain_collator_peer_count",
					"Amount of collator peers connected",
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

struct CollatingPeerState {
	collator_id: CollatorId,
	para_id: ParaId,
	// Advertised relay parents.
	advertisements: HashSet<Hash>,
	last_active: Instant,
}

enum PeerState {
	// The peer has connected at the given instant.
	Connected(Instant),
	// Thepe
	Collating(CollatingPeerState),
}

#[derive(Debug)]
enum AdvertisementError {
	Duplicate,
	OutOfOurView,
	UndeclaredCollator,
}

struct PeerData {
	view: View,
	state: PeerState,
}

impl PeerData {
	fn new(view: View) -> Self {
		PeerData {
			view,
			state: PeerState::Connected(Instant::now()),
		}
	}

	/// Update the view, clearing all advertisements that are no longer in the
	/// current view.
	fn update_view(&mut self, new_view: View) {
		let old_view = std::mem::replace(&mut self.view, new_view);
		if let PeerState::Collating(ref mut peer_state) = self.state {
			for removed in old_view.difference(&self.view) {
				let _ = peer_state.advertisements.remove(&removed);
			}
		}
	}

	/// Prune old advertisements relative to our view.
	fn prune_old_advertisements(&mut self, our_view: &View) {
		if let PeerState::Collating(ref mut peer_state) = self.state {
			peer_state.advertisements.retain(|a| our_view.contains(a));
		}
	}

	/// Note an advertisement by the collator. Returns `true` if the advertisement was imported
	/// successfully. Fails if the advertisement is duplicate, out of view, or the peer has not
	/// declared itself a collator.
	fn insert_advertisement(
		&mut self,
		on_relay_parent: Hash,
		our_view: &View,
	)
		-> std::result::Result<(CollatorId, ParaId), AdvertisementError>
	{
		match self.state {
			PeerState::Connected(_) => Err(AdvertisementError::UndeclaredCollator),
			_ if !our_view.contains(&on_relay_parent) => Err(AdvertisementError::OutOfOurView),
			PeerState::Collating(ref mut state) => {
				if state.advertisements.insert(on_relay_parent) {
					state.last_active = Instant::now();
					Ok((state.collator_id.clone(), state.para_id.clone()))
				} else {
					Err(AdvertisementError::Duplicate)
				}
			}
		}
	}

	/// Whether a peer is collating.
	fn is_collating(&self) -> bool {
		match self.state {
			PeerState::Connected(_) => false,
			PeerState::Collating(_) => true,
		}
	}

	/// Note that a peer is now collating with the given collator and para ids.
	///
	/// This will overwrite any previous call to `set_collating` and should only be called
	/// if `is_collating` is false.
	fn set_collating(&mut self, collator_id: CollatorId, para_id: ParaId) {
		self.state = PeerState::Collating(CollatingPeerState {
			collator_id,
			para_id,
			advertisements: HashSet::new(),
			last_active: Instant::now(),
		});
	}

	fn collator_id(&self) -> Option<&CollatorId> {
		match self.state {
			PeerState::Connected(_) => None,
			PeerState::Collating(ref state) => Some(&state.collator_id),
		}
	}

	fn collating_para(&self) -> Option<ParaId> {
		match self.state {
			PeerState::Connected(_) => None,
			PeerState::Collating(ref state) => Some(state.para_id),
		}
	}

	/// Whether the peer has advertised the given collation.
	fn has_advertised(&self, relay_parent: &Hash) -> bool {
		match self.state {
			PeerState::Connected(_) => false,
			PeerState::Collating(ref state) => state.advertisements.contains(relay_parent),
		}
	}

	/// Whether the peer is now inactive according to the current instant and the eviction policy.
	fn is_inactive(&self, now: Instant, policy: &crate::CollatorEvictionPolicy) -> bool {
		match self.state {
			PeerState::Connected(connected_at) => connected_at + policy.undeclared < now,
			PeerState::Collating(ref state) => state.last_active + policy.inactive_collator < now,
		}
	}
}

impl Default for PeerData {
	fn default() -> Self {
		PeerData::new(Default::default())
	}
}

struct GroupAssignments {
	current: Option<ParaId>,
	next: Option<ParaId>,
}

#[derive(Default)]
struct ActiveParas {
	relay_parent_assignments: HashMap<Hash, GroupAssignments>,
	current_assignments: HashMap<ParaId, usize>,
	next_assignments: HashMap<ParaId, usize>
}

impl ActiveParas {
	async fn assign_incoming(
		&mut self,
		sender: &mut impl SubsystemSender,
		keystore: &SyncCryptoStorePtr,
		new_relay_parents: impl IntoIterator<Item = Hash>,
	) {
		for relay_parent in new_relay_parents {
			let mv = polkadot_node_subsystem_util::request_validators(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();

			let mg = polkadot_node_subsystem_util::request_validator_groups(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();


			let mc = polkadot_node_subsystem_util::request_availability_cores(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();

			let (validators, groups, rotation_info, cores) = match (mv, mg, mc) {
				(Some(v), Some((g, r)), Some(c)) => (v, g, r, c),
				_ => {
					tracing::debug!(
						target: LOG_TARGET,
						relay_parent = ?relay_parent,
						"Failed to query runtime API for relay-parent",
					);

					continue
				}
			};

			let (para_now, para_next) = match polkadot_node_subsystem_util
				::signing_key_and_index(&validators, keystore)
				.await
				.and_then(|(_, index)| polkadot_node_subsystem_util::find_validator_group(
					&groups,
					index,
				))
			{
				Some(group) => {
					let next_rotation_info = rotation_info.bump_rotation();

					let core_now = rotation_info.core_for_group(group, cores.len());
					let core_next = next_rotation_info.core_for_group(group, cores.len());

					(
						cores.get(core_now.0 as usize).and_then(|c| c.para_id()),
						cores.get(core_next.0 as usize).and_then(|c| c.para_id()),
					)
				}
				None => {
					tracing::trace!(
						target: LOG_TARGET,
						relay_parent = ?relay_parent,
						"Not a validator",
					);

					continue
				}
			};

			// This code won't work well, if at all for parathreads. For parathreads we'll
			// have to be aware of which core the parathread claim is going to be multiplexed
			// onto. The parathread claim will also have a known collator, and we should always
			// allow an incoming connection from that collator. If not even connecting to them
			// directly.
			//
			// However, this'll work fine for parachains, as each parachain gets a dedicated
			// core.
			if let Some(para_now) = para_now {
				*self.current_assignments.entry(para_now).or_default() += 1;
			}

			if let Some(para_next) = para_next {
				*self.next_assignments.entry(para_next).or_default() += 1;
			}

			self.relay_parent_assignments.insert(
				relay_parent,
				GroupAssignments { current: para_now, next: para_next },
			);
		}
	}

	fn remove_outgoing(
		&mut self,
		old_relay_parents: impl IntoIterator<Item = Hash>,
	) {
		for old_relay_parent in old_relay_parents {
			if let Some(assignments) = self.relay_parent_assignments.remove(&old_relay_parent) {
				let GroupAssignments { current, next } = assignments;

				if let Some(cur) = current {
					if let Entry::Occupied(mut occupied) = self.current_assignments.entry(cur) {
						*occupied.get_mut() -= 1;
						if *occupied.get() == 0 {
							occupied.remove_entry();
						}
					}
				}

				if let Some(next) = next {
					if let Entry::Occupied(mut occupied) = self.next_assignments.entry(next) {
						*occupied.get_mut() -= 1;
						if *occupied.get() == 0 {
							occupied.remove_entry();
						}
					}
				}
			}
		}
	}

	fn is_current_or_next(&self, id: ParaId) -> bool {
		self.current_assignments.contains_key(&id) || self.next_assignments.contains_key(&id)
	}
}

/// All state relevant for the validator side of the protocol lives here.
#[derive(Default)]
struct State {
	/// Our own view.
	view: OurView,

	/// Active paras based on our view. We only accept collators from these paras.
	active_paras: ActiveParas,

	/// Track all active collators and their data.
	peer_data: HashMap<PeerId, PeerData>,

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

// O(n) search for collator ID by iterating through the peers map. This should be fast enough
// unless a large amount of peers is expected.
fn collator_peer_id(
	peer_data: &HashMap<PeerId, PeerData>,
	collator_id: &CollatorId,
) -> Option<PeerId> {
	peer_data.iter()
		.find_map(|(peer, data)|
			data.collator_id().filter(|c| c == &collator_id).map(|_| peer.clone())
		)
}

async fn disconnect_peer(ctx: &mut impl SubsystemContext, peer_id: PeerId) {
	ctx.send_message(
		NetworkBridgeMessage::DisconnectPeer(peer_id, PeerSet::Collation).into()
	).await
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
	let peer_id = match collator_peer_id(&state.peer_data, &collator_id) {
		None => return,
		Some(p) => p,
	};

	if state.peer_data.get(&peer_id).map_or(false, |d| d.has_advertised(&relay_parent)) {
		request_collation(ctx, state, relay_parent, para_id, peer_id, tx).await;
	}
}

/// Report a collator for some malicious actions.
#[tracing::instrument(level = "trace", skip(ctx, peer_data), fields(subsystem = LOG_TARGET))]
async fn report_collator<Context>(
	ctx: &mut Context,
	peer_data: &HashMap<PeerId, PeerData>,
	id: CollatorId,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	if let Some(peer_id) = collator_peer_id(peer_data, &id) {
		modify_reputation(ctx, peer_id, COST_REPORT_BAD).await;
	}
}

/// Some other subsystem has reported a collator as a good one, bump reputation.
#[tracing::instrument(level = "trace", skip(ctx, peer_data), fields(subsystem = LOG_TARGET))]
async fn note_good_collation<Context>(
	ctx: &mut Context,
	peer_data: &HashMap<PeerId, PeerData>,
	id: CollatorId,
)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	if let Some(peer_id) = collator_peer_id(peer_data, &id) {
		modify_reputation(ctx, peer_id, BENEFIT_NOTIFY_GOOD).await;
	}
}

/// Notify a collator that its collation got seconded.
#[tracing::instrument(level = "trace", skip(ctx, peer_data), fields(subsystem = LOG_TARGET))]
async fn notify_collation_seconded(
	ctx: &mut impl SubsystemContext<Message = CollatorProtocolMessage>,
	peer_data: &HashMap<PeerId, PeerData>,
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

	if let Some(peer_id) = collator_peer_id(peer_data, &id) {
		let wire_message = protocol_v1::CollatorProtocolMessage::CollationSeconded(statement);

		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendCollationMessage(
				vec![peer_id],
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
	let peer_data = state.peer_data.entry(peer_id.clone()).or_default();

	peer_data.update_view(view);
	state.requested_collations
		.retain(|(rp, _, pid), _| pid != &peer_id || !peer_data.has_advertised(&rp));

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

	match msg {
		Declare(collator_id, para_id, signature) => {
			if collator_peer_id(&state.peer_data, &collator_id).is_some() {
				modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			let peer_data = match state.peer_data.get_mut(&origin) {
				Some(p) => p,
				None => {
					modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
					return
				}
			};

			if peer_data.is_collating() {
				modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			if !signature.verify(&*protocol_v1::declare_signature_payload(&origin), &collator_id) {
				modify_reputation(ctx, origin, COST_INVALID_SIGNATURE).await;
				return
			}

			if state.active_paras.is_current_or_next(para_id) {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?collator_id,
					?para_id,
					"Declared as collator for current or next para",
				);

				peer_data.set_collating(collator_id, para_id);
			} else {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?collator_id,
					?para_id,
					"Declared as collator for unneeded para",
				);

				modify_reputation(ctx, origin.clone(), COST_UNNEEDED_COLLATOR).await;
				disconnect_peer(ctx, origin).await;
			}
		}
		AdvertiseCollation(relay_parent) => {
			let _span = state.span_per_relay_parent.get(&relay_parent).map(|s| s.child("advertise-collation"));

			if !state.view.contains(&relay_parent) {
				tracing::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?relay_parent,
					"Advertise collation out of view",
				);

				modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				return;
			}

			let peer_data = match state.peer_data.get_mut(&origin) {
				None => {
					modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
					return;
				}
				Some(p) => p,
			};

			match peer_data.insert_advertisement(relay_parent, &state.view) {
				Ok((collator_id, para_id)) => {
					tracing::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						%para_id,
						?relay_parent,
						"Received advertise collation",
					);

					notify_candidate_selection(ctx, collator_id, relay_parent, para_id).await;
				}
				Err(e) => {
					tracing::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						?relay_parent,
						error = ?e,
						"Invalid advertisement",
					);

					modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				}
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
#[tracing::instrument(level = "trace", skip(ctx, state, keystore), fields(subsystem = LOG_TARGET))]
async fn handle_our_view_change(
	ctx: &mut impl SubsystemContext,
	state: &mut State,
	keystore: &SyncCryptoStorePtr,
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

	let added = state.view.difference(&old_view).cloned().collect::<Vec<_>>();
	let removed = old_view
		.difference(&state.view)
		.cloned()
		.collect::<Vec<_>>();

	for removed in removed.iter().cloned() {
		remove_relay_parent(state, removed).await?;
		state.span_per_relay_parent.remove(&removed);
	}

	state.active_paras.assign_incoming(ctx.sender(), keystore, added).await;
	state.active_paras.remove_outgoing(removed);

	for (peer_id, peer_data) in state.peer_data.iter_mut() {
		peer_data.prune_old_advertisements(&state.view);

		// Disconnect peers who are not relevant to our current or next para.
		//
		// If the peer hasn't declared yet, they will be disconnected if they do not
		// declare.
		if let Some(para_id) = peer_data.collating_para() {
			if !state.active_paras.is_current_or_next(para_id) {
				disconnect_peer(ctx, peer_id.clone()).await;
			}
		}
	}

	Ok(())
}

/// Bridge event switch.
#[tracing::instrument(level = "trace", skip(ctx, state, keystore), fields(subsystem = LOG_TARGET))]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut State,
	keystore: &SyncCryptoStorePtr,
	bridge_message: NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(peer_id, _role, _) => {
			state.peer_data.entry(peer_id).or_default();
			state.metrics.note_collator_peer_count(state.peer_data.len());
		},
		PeerDisconnected(peer_id) => {
			state.peer_data.remove(&peer_id);
			state.metrics.note_collator_peer_count(state.peer_data.len());
		},
		PeerViewChange(peer_id, view) => {
			handle_peer_view_change(state, peer_id, view).await?;
		},
		OurViewChange(view) => {
			handle_our_view_change(ctx, state, keystore, view).await?;
		},
		PeerMessage(remote, msg) => {
			process_incoming_peer_message(ctx, state, remote, msg).await;
		}
	}

	Ok(())
}

/// The main message receiver switch.
#[tracing::instrument(level = "trace", skip(ctx, keystore, state), fields(subsystem = LOG_TARGET))]
async fn process_msg<Context>(
	ctx: &mut Context,
	keystore: &SyncCryptoStorePtr,
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
			report_collator(ctx, &state.peer_data, id).await;
		}
		NoteGoodCollation(id) => {
			note_good_collation(ctx, &state.peer_data, id).await;
		}
		NotifyCollationSeconded(id, statement) => {
			notify_collation_seconded(ctx, &state.peer_data, id, statement).await;
		}
		NetworkBridgeUpdateV1(event) => {
			if let Err(e) = handle_network_msg(
				ctx,
				state,
				keystore,
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
#[tracing::instrument(skip(ctx, keystore, metrics), fields(subsystem = LOG_TARGET))]
pub(crate) async fn run<Context>(
	mut ctx: Context,
	keystore: SyncCryptoStorePtr,
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
					Communication { msg } => process_msg(
						&mut ctx,
						&keystore,
						msg,
						&mut state,
					).await,
					Signal(BlockFinalized(..)) => {}
					Signal(ActiveLeaves(_)) => {}
					Signal(Conclude) => { break }
				}

				continue
			}
			Some(Either::Right(())) => {
				disconnect_inactive_peers(&mut ctx, &eviction_policy, &state.peer_data).await;
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
	eviction_policy: &crate::CollatorEvictionPolicy,
	peers: &HashMap<PeerId, PeerData>,
) {
	let now = Instant::now();
	for (peer, peer_data) in peers {
		if peer_data.is_inactive(now, &eviction_policy) {
			disconnect_peer(ctx, peer.clone()).await;
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
			Ok(CollationFetchingResponse::Collation(receipt, _))
				if receipt.descriptor().para_id != *para_id =>
			{
				tracing::debug!(
					target: LOG_TARGET,
					expected_para_id = ?para_id,
					got_para_id = ?receipt.descriptor().para_id,
					peer_id = ?peer_id,
					"Got wrong para ID for requested collation."
				);

				modify_reputation(ctx, *peer_id, COST_WRONG_PARA).await;
			}
			Ok(CollationFetchingResponse::Collation(receipt, pov)) => {
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
	use std::sync::Arc;
	use futures::{executor, future, Future};
	use sp_core::{crypto::Pair, Encode};
	use sp_keystore::SyncCryptoStore;
	use sp_keystore::testing::KeyStore as TestKeyStore;
	use sp_keyring::Sr25519Keyring;
	use assert_matches::assert_matches;

	use polkadot_primitives::v1::{
		CollatorPair, ValidatorId, ValidatorIndex, CoreState, CandidateDescriptor,
		GroupRotationInfo, ScheduledCore, OccupiedCore, GroupIndex,
	};
	use polkadot_node_primitives::BlockData;
	use polkadot_node_subsystem_util::TimeoutExt;
	use polkadot_subsystem_testhelpers as test_helpers;
	use polkadot_subsystem::messages::{RuntimeApiMessage, RuntimeApiRequest};
	use polkadot_node_network_protocol::{our_view, ObservedRole,
		request_response::Requests
	};

	const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(50);
	const DECLARE_TIMEOUT: Duration = Duration::from_millis(25);

	#[derive(Clone)]
	struct TestState {
		chain_ids: Vec<ParaId>,
		relay_parent: Hash,
		collators: Vec<CollatorPair>,
		validators: Vec<Sr25519Keyring>,
		validator_public: Vec<ValidatorId>,
		validator_groups: Vec<Vec<ValidatorIndex>>,
		group_rotation_info: GroupRotationInfo,
		cores: Vec<CoreState>,
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

			let validators = vec![
				Sr25519Keyring::Alice,
				Sr25519Keyring::Bob,
				Sr25519Keyring::Charlie,
				Sr25519Keyring::Dave,
				Sr25519Keyring::Eve,
			];

			let validator_public = validators.iter().map(|k| k.public().into()).collect();
			let validator_groups = vec![
				vec![ValidatorIndex(0), ValidatorIndex(1)],
				vec![ValidatorIndex(2), ValidatorIndex(3)],
				vec![ValidatorIndex(4)],
			];

			let group_rotation_info = GroupRotationInfo {
				session_start_block: 0,
				group_rotation_frequency: 1,
				now: 0,
			};

			let cores = vec![
				CoreState::Scheduled(ScheduledCore {
					para_id: chain_ids[0],
					collator: None,
				}),
				CoreState::Free,
				CoreState::Occupied(OccupiedCore {
					next_up_on_available: None,
					occupied_since: 0,
					time_out_at: 1,
					next_up_on_time_out: None,
					availability: Default::default(),
					group_responsible: GroupIndex(0),
					candidate_hash: Default::default(),
					candidate_descriptor: {
						let mut d = CandidateDescriptor::default();
						d.para_id = chain_ids[1];

						d
					},
				}),
			];

			Self {
				chain_ids,
				relay_parent,
				collators,
				validators,
				validator_public,
				validator_groups,
				group_rotation_info,
				cores,
			}
		}
	}

	type VirtualOverseer = test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>;

	struct TestHarness {
		virtual_overseer: VirtualOverseer,
	}

	fn test_harness<T: Future<Output = VirtualOverseer>>(test: impl FnOnce(TestHarness) -> T) {
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

		let keystore = TestKeyStore::new();
		keystore.sr25519_generate_new(
			polkadot_primitives::v1::PARACHAIN_KEY_TYPE_ID,
			Some(&Sr25519Keyring::Alice.to_seed()),
		).unwrap();

		let subsystem = run(
			context,
			Arc::new(keystore),
			crate::CollatorEvictionPolicy {
				inactive_collator: ACTIVITY_TIMEOUT,
				undeclared: DECLARE_TIMEOUT,
			},
			Metrics::default(),
		);

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::join(async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		}, subsystem)).1.unwrap();
	}

	const TIMEOUT: Duration = Duration::from_millis(200);

	async fn overseer_send(
		overseer: &mut VirtualOverseer,
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
		overseer: &mut VirtualOverseer,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

		tracing::trace!("Received message:\n{:?}", &msg);

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut VirtualOverseer,
		timeout: Duration,
	) -> Option<AllMessages> {
		tracing::trace!("Waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	async fn overseer_signal(
		overseer: &mut VirtualOverseer,
		signal: OverseerSignal,
	) {
		overseer
			.send(FromOverseer::Signal(signal))
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
	}

	async fn respond_to_core_info_queries(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
	) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::Validators(tx),
			)) => {
				let _ = tx.send(Ok(test_state.validator_public.clone()));
			}
		);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::ValidatorGroups(tx),
			)) => {
				let _ = tx.send(Ok((
					test_state.validator_groups.clone(),
					test_state.group_rotation_info.clone(),
				)));
			}
		);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::AvailabilityCores(tx),
			)) => {
				let _ = tx.send(Ok(test_state.cores.clone()));
			}
		);
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

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							pair.public(),
							test_state.chain_ids[0],
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
			virtual_overseer
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

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_c,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
							test_state.chain_ids[0],
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
							test_state.chain_ids[0],
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
			virtual_overseer
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

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			// the peer sends a declare message but sign the wrong payload
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[0].public(),
						test_state.chain_ids[0],
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
			virtual_overseer
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

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_c,
						ObservedRole::Full,
						None,
					),
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
							test_state.chain_ids[0],
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
							test_state.chain_ids[0],
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
					PoV {
						block_data: BlockData(vec![]),
					},
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
					PoV {
						block_data: BlockData(vec![1, 2, 3]),
					},
				).encode()
			)).expect("Sending response should succeed");

			let collation_0 = rx_0.await.unwrap();
			let collation_1 = rx_1.await.unwrap();

			assert_eq!(collation_0.0, candidate_a);
			assert_eq!(collation_1.0, candidate_b);
			virtual_overseer
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

			let hash_a = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_a])
				)
			).await;

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						None,
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
							test_state.chain_ids[0],
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
			);
			virtual_overseer
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

			let hash_a = test_state.relay_parent;
			let hash_b = Hash::repeat_byte(1);
			let hash_c = Hash::repeat_byte(2);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_a, hash_b, hash_c])
				)
			).await;

			// 3 heads, 3 times.
			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						None,
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
							test_state.chain_ids[0],
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
							hash_a,
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
							hash_b
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
							hash_c,
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
				assert_eq!(relay_parent, hash_c);
				assert_eq!(para_id, test_state.chain_ids[0]);
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
			);
			virtual_overseer
		});
	}

	#[test]
	fn disconnect_if_no_declare() {
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

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						None,
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					peer,
					peer_set,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(peer_set, PeerSet::Collation);
				}
			);
			virtual_overseer
		})
	}

	#[test]
	fn disconnect_if_wrong_declare() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
				)
			).await;

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						None,
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
							ParaId::from(69),
							pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
					peer,
					rep,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_UNNEEDED_COLLATOR);
				}
			);

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					peer,
					peer_set,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(peer_set, PeerSet::Collation);
				}
			);
			virtual_overseer
		})
	}

	#[test]
	fn view_change_clears_old_collators() {
		let mut test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
				)
			).await;

			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						None,
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
							test_state.chain_ids[0],
							pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
						)
					)
				)
			).await;

			let hash_b = Hash::repeat_byte(69);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(our_view![hash_b])
				)
			).await;

			test_state.group_rotation_info = test_state.group_rotation_info.bump_rotation();
			respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
					peer,
					peer_set,
				)) => {
					assert_eq!(peer, peer_b);
					assert_eq!(peer_set, PeerSet::Collation);
				}
			);
			virtual_overseer
		})
	}
}
