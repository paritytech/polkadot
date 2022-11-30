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

use always_assert::never;
use futures::{
	channel::oneshot,
	future::{BoxFuture, Fuse, FusedFuture},
	select,
	stream::FuturesUnordered,
	FutureExt, StreamExt,
};
use futures_timer::Delay;
use std::{
	collections::{hash_map::Entry, HashMap, HashSet},
	sync::Arc,
	task::Poll,
	time::{Duration, Instant},
};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::PeerSet,
	request_response as req_res,
	request_response::{
		outgoing::{Recipient, RequestError},
		v1::{CollationFetchingRequest, CollationFetchingResponse},
		OutgoingRequest, Requests,
	},
	v1 as protocol_v1, OurView, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{PoV, SignedFullStatement};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		CandidateBackingMessage, CollatorProtocolMessage, IfDisconnected, NetworkBridgeEvent,
		NetworkBridgeTxMessage, RuntimeApiMessage,
	},
	overseer, FromOrchestra, OverseerSignal, PerLeafSpan, SubsystemSender,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_primitives::v2::{CandidateReceipt, CollatorId, Hash, Id as ParaId};

use crate::error::Result;

use super::{modify_reputation, tick_stream, LOG_TARGET};

#[cfg(test)]
mod tests;

const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("An unexpected message");
/// Message could not be decoded properly.
const COST_CORRUPTED_MESSAGE: Rep = Rep::CostMinor("Message was corrupt");
/// Network errors that originated at the remote host should have same cost as timeout.
const COST_NETWORK_ERROR: Rep = Rep::CostMinor("Some network error");
const COST_INVALID_SIGNATURE: Rep = Rep::Malicious("Invalid network message signature");
const COST_REPORT_BAD: Rep = Rep::Malicious("A collator was reported by another subsystem");
const COST_WRONG_PARA: Rep = Rep::Malicious("A collator provided a collation for the wrong para");
const COST_UNNEEDED_COLLATOR: Rep = Rep::CostMinor("An unneeded collator connected");
const BENEFIT_NOTIFY_GOOD: Rep =
	Rep::BenefitMinor("A collator was noted good by another subsystem");

/// Time after starting a collation download from a collator we will start another one from the
/// next collator even if the upload was not finished yet.
///
/// This is to protect from a single slow collator preventing collations from happening.
///
/// With a collation size of 5MB and bandwidth of 500Mbit/s (requirement for Kusama validators),
/// the transfer should be possible within 0.1 seconds. 400 milliseconds should therefore be
/// plenty, even with multiple heads and should be low enough for later collators to still be able
/// to finish on time.
///
/// There is debug logging output, so we can adjust this value based on production results.
#[cfg(not(test))]
const MAX_UNSHARED_DOWNLOAD_TIME: Duration = Duration::from_millis(400);

// How often to check all peers with activity.
#[cfg(not(test))]
const ACTIVITY_POLL: Duration = Duration::from_secs(1);

#[cfg(test)]
const MAX_UNSHARED_DOWNLOAD_TIME: Duration = Duration::from_millis(100);

#[cfg(test)]
const ACTIVITY_POLL: Duration = Duration::from_millis(10);

// How often to poll collation responses.
// This is a hack that should be removed in a refactoring.
// See https://github.com/paritytech/polkadot/issues/4182
const CHECK_COLLATIONS_POLL: Duration = Duration::from_millis(50);

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
	fn time_handle_collation_request_result(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.handle_collation_request_result.start_timer())
	}

	/// Note the current number of collator peers.
	fn note_collator_peer_count(&self, collator_peers: usize) {
		self.0
			.as_ref()
			.map(|metrics| metrics.collator_peer_count.set(collator_peers as u64));
	}

	/// Provide a timer for `PerRequest` structure which observes on drop.
	fn time_collation_request_duration(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.collation_request_duration.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	collation_requests: prometheus::CounterVec<prometheus::U64>,
	process_msg: prometheus::Histogram,
	handle_collation_request_result: prometheus::Histogram,
	collator_peer_count: prometheus::Gauge<prometheus::U64>,
	collation_request_duration: prometheus::Histogram,
}

impl metrics::Metrics for Metrics {
	fn try_register(
		registry: &prometheus::Registry,
	) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			collation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_collation_requests_total",
						"Number of collations requested from Collators.",
					),
					&["success"],
				)?,
				registry,
			)?,
			process_msg: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_process_msg",
						"Time spent within `collator_protocol_validator::process_msg`",
					)
				)?,
				registry,
			)?,
			handle_collation_request_result: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_handle_collation_request_result",
						"Time spent within `collator_protocol_validator::handle_collation_request_result`",
					)
				)?,
				registry,
			)?,
			collator_peer_count: prometheus::register(
				prometheus::Gauge::new(
					"polkadot_parachain_collator_peer_count",
					"Amount of collator peers connected",
				)?,
				registry,
			)?,
			collation_request_duration: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_collation_request_duration",
						"Lifetime of the `PerRequest` structure",
					).buckets(vec![0.05, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0, 1.2, 1.5, 1.75]),
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
	/// A metric histogram for the lifetime of the request
	_lifetime_timer: Option<metrics::prometheus::prometheus::HistogramTimer>,
}

#[derive(Debug)]
struct CollatingPeerState {
	collator_id: CollatorId,
	para_id: ParaId,
	// Advertised relay parents.
	advertisements: HashSet<Hash>,
	last_active: Instant,
}

#[derive(Debug)]
enum PeerState {
	// The peer has connected at the given instant.
	Connected(Instant),
	// Peer is collating.
	Collating(CollatingPeerState),
}

#[derive(Debug)]
enum AdvertisementError {
	Duplicate,
	OutOfOurView,
	UndeclaredCollator,
}

#[derive(Debug)]
struct PeerData {
	view: View,
	state: PeerState,
}

impl PeerData {
	fn new(view: View) -> Self {
		PeerData { view, state: PeerState::Connected(Instant::now()) }
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
	) -> std::result::Result<(CollatorId, ParaId), AdvertisementError> {
		match self.state {
			PeerState::Connected(_) => Err(AdvertisementError::UndeclaredCollator),
			_ if !our_view.contains(&on_relay_parent) => Err(AdvertisementError::OutOfOurView),
			PeerState::Collating(ref mut state) =>
				if state.advertisements.insert(on_relay_parent) {
					state.last_active = Instant::now();
					Ok((state.collator_id.clone(), state.para_id))
				} else {
					Err(AdvertisementError::Duplicate)
				},
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
	fn is_inactive(&self, policy: &crate::CollatorEvictionPolicy) -> bool {
		match self.state {
			PeerState::Connected(connected_at) => connected_at.elapsed() >= policy.undeclared,
			PeerState::Collating(ref state) =>
				state.last_active.elapsed() >= policy.inactive_collator,
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
}

#[derive(Default)]
struct ActiveParas {
	relay_parent_assignments: HashMap<Hash, GroupAssignments>,
	current_assignments: HashMap<ParaId, usize>,
}

impl ActiveParas {
	async fn assign_incoming(
		&mut self,
		sender: &mut impl SubsystemSender<RuntimeApiMessage>,
		keystore: &SyncCryptoStorePtr,
		new_relay_parents: impl IntoIterator<Item = Hash>,
	) {
		for relay_parent in new_relay_parents {
			let mv = polkadot_node_subsystem_util::request_validators(relay_parent, sender)
				.await
				.await
				.ok()
				.and_then(|x| x.ok());

			let mg = polkadot_node_subsystem_util::request_validator_groups(relay_parent, sender)
				.await
				.await
				.ok()
				.and_then(|x| x.ok());

			let mc = polkadot_node_subsystem_util::request_availability_cores(relay_parent, sender)
				.await
				.await
				.ok()
				.and_then(|x| x.ok());

			let (validators, groups, rotation_info, cores) = match (mv, mg, mc) {
				(Some(v), Some((g, r)), Some(c)) => (v, g, r, c),
				_ => {
					gum::debug!(
						target: LOG_TARGET,
						?relay_parent,
						"Failed to query runtime API for relay-parent",
					);

					continue
				},
			};

			let para_now =
				match polkadot_node_subsystem_util::signing_key_and_index(&validators, keystore)
					.await
					.and_then(|(_, index)| {
						polkadot_node_subsystem_util::find_validator_group(&groups, index)
					}) {
					Some(group) => {
						let core_now = rotation_info.core_for_group(group, cores.len());

						cores.get(core_now.0 as usize).and_then(|c| c.para_id())
					},
					None => {
						gum::trace!(target: LOG_TARGET, ?relay_parent, "Not a validator");

						continue
					},
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
				let entry = self.current_assignments.entry(para_now).or_default();
				*entry += 1;
				if *entry == 1 {
					gum::debug!(
						target: LOG_TARGET,
						?relay_parent,
						para_id = ?para_now,
						"Assigned to a parachain",
					);
				}
			}

			self.relay_parent_assignments
				.insert(relay_parent, GroupAssignments { current: para_now });
		}
	}

	fn remove_outgoing(&mut self, old_relay_parents: impl IntoIterator<Item = Hash>) {
		for old_relay_parent in old_relay_parents {
			if let Some(assignments) = self.relay_parent_assignments.remove(&old_relay_parent) {
				let GroupAssignments { current } = assignments;

				if let Some(cur) = current {
					if let Entry::Occupied(mut occupied) = self.current_assignments.entry(cur) {
						*occupied.get_mut() -= 1;
						if *occupied.get() == 0 {
							occupied.remove_entry();
							gum::debug!(
								target: LOG_TARGET,
								para_id = ?cur,
								"Unassigned from a parachain",
							);
						}
					}
				}
			}
		}
	}

	fn is_current(&self, id: &ParaId) -> bool {
		self.current_assignments.contains_key(id)
	}
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct PendingCollation {
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	commitments_hash: Option<Hash>,
}

impl PendingCollation {
	fn new(relay_parent: Hash, para_id: &ParaId, peer_id: &PeerId) -> Self {
		Self { relay_parent, para_id: *para_id, peer_id: *peer_id, commitments_hash: None }
	}
}

type CollationEvent = (CollatorId, PendingCollation);

type PendingCollationFetch =
	(CollationEvent, std::result::Result<(CandidateReceipt, PoV), oneshot::Canceled>);

/// The status of the collations in [`CollationsPerRelayParent`].
#[derive(Debug, Clone, Copy)]
enum CollationStatus {
	/// We are waiting for a collation to be advertised to us.
	Waiting,
	/// We are currently fetching a collation.
	Fetching,
	/// We are waiting that a collation is being validated.
	WaitingOnValidation,
	/// We have seconded a collation.
	Seconded,
}

impl Default for CollationStatus {
	fn default() -> Self {
		Self::Waiting
	}
}

impl CollationStatus {
	/// Downgrades to `Waiting`, but only if `self != Seconded`.
	fn back_to_waiting(&mut self) {
		match self {
			Self::Seconded => {},
			_ => *self = Self::Waiting,
		}
	}
}

/// Information about collations per relay parent.
#[derive(Default)]
struct CollationsPerRelayParent {
	/// What is the current status in regards to a collation for this relay parent?
	status: CollationStatus,
	/// Collation currently being fetched.
	///
	/// This is the currently last started fetch, which did not exceed `MAX_UNSHARED_DOWNLOAD_TIME`
	/// yet.
	waiting_collation: Option<CollatorId>,
	/// Collation that were advertised to us, but we did not yet fetch.
	unfetched_collations: Vec<(PendingCollation, CollatorId)>,
}

impl CollationsPerRelayParent {
	/// Returns the next collation to fetch from the `unfetched_collations`.
	///
	/// This will reset the status back to `Waiting` using [`CollationStatus::back_to_waiting`].
	///
	/// Returns `Some(_)` if there is any collation to fetch, the `status` is not `Seconded` and
	/// the passed in `finished_one` is the currently `waiting_collation`.
	pub fn get_next_collation_to_fetch(
		&mut self,
		finished_one: Option<&CollatorId>,
	) -> Option<(PendingCollation, CollatorId)> {
		// If finished one does not match waiting_collation, then we already dequeued another fetch
		// to replace it.
		if self.waiting_collation.as_ref() != finished_one {
			gum::trace!(
				target: LOG_TARGET,
				waiting_collation = ?self.waiting_collation,
				?finished_one,
				"Not proceeding to the next collation - has already been done."
			);
			return None
		}
		self.status.back_to_waiting();

		match self.status {
			// We don't need to fetch any other collation when we already have seconded one.
			CollationStatus::Seconded => None,
			CollationStatus::Waiting => {
				let next = self.unfetched_collations.pop();
				self.waiting_collation = next.as_ref().map(|(_, collator_id)| collator_id.clone());
				next
			},
			CollationStatus::WaitingOnValidation | CollationStatus::Fetching =>
				unreachable!("We have reset the status above!"),
		}
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
	requested_collations: HashMap<PendingCollation, PerRequest>,

	/// Metrics.
	metrics: Metrics,

	/// Span per relay parent.
	span_per_relay_parent: HashMap<Hash, PerLeafSpan>,

	/// Keep track of all fetch collation requests
	collation_fetches: FuturesUnordered<BoxFuture<'static, PendingCollationFetch>>,

	/// When a timer in this `FuturesUnordered` triggers, we should dequeue the next request
	/// attempt in the corresponding `collations_per_relay_parent`.
	///
	/// A triggering timer means that the fetching took too long for our taste and we should give
	/// another collator the chance to be faster (dequeue next fetch request as well).
	collation_fetch_timeouts: FuturesUnordered<BoxFuture<'static, (CollatorId, Hash)>>,

	/// Information about the collations per relay parent.
	collations_per_relay_parent: HashMap<Hash, CollationsPerRelayParent>,

	/// Keep track of all pending candidate collations
	pending_candidates: HashMap<Hash, CollationEvent>,
}

// O(n) search for collator ID by iterating through the peers map. This should be fast enough
// unless a large amount of peers is expected.
fn collator_peer_id(
	peer_data: &HashMap<PeerId, PeerData>,
	collator_id: &CollatorId,
) -> Option<PeerId> {
	peer_data
		.iter()
		.find_map(|(peer, data)| data.collator_id().filter(|c| c == &collator_id).map(|_| *peer))
}

async fn disconnect_peer(sender: &mut impl overseer::CollatorProtocolSenderTrait, peer_id: PeerId) {
	sender
		.send_message(NetworkBridgeTxMessage::DisconnectPeer(peer_id, PeerSet::Collation))
		.await
}

/// Another subsystem has requested to fetch collations on a particular leaf for some para.
async fn fetch_collation(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	state: &mut State,
	pc: PendingCollation,
	id: CollatorId,
) {
	let (tx, rx) = oneshot::channel();

	let PendingCollation { relay_parent, para_id, peer_id, .. } = pc;

	let timeout = |collator_id, relay_parent| async move {
		Delay::new(MAX_UNSHARED_DOWNLOAD_TIME).await;
		(collator_id, relay_parent)
	};
	state.collation_fetch_timeouts.push(timeout(id.clone(), relay_parent).boxed());

	if let Some(peer_data) = state.peer_data.get(&peer_id) {
		if peer_data.has_advertised(&relay_parent) {
			request_collation(sender, state, relay_parent, para_id, peer_id, tx).await;
		} else {
			gum::debug!(
				target: LOG_TARGET,
				?peer_id,
				?para_id,
				?relay_parent,
				"Collation is not advertised for the relay parent by the peer, do not request it",
			);
		}
	} else {
		gum::warn!(
			target: LOG_TARGET,
			?peer_id,
			?para_id,
			?relay_parent,
			"Requested to fetch a collation from an unknown peer",
		);
	}

	state.collation_fetches.push(rx.map(|r| ((id, pc), r)).boxed());
}

/// Report a collator for some malicious actions.
async fn report_collator(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	peer_data: &HashMap<PeerId, PeerData>,
	id: CollatorId,
) {
	if let Some(peer_id) = collator_peer_id(peer_data, &id) {
		modify_reputation(sender, peer_id, COST_REPORT_BAD).await;
	}
}

/// Some other subsystem has reported a collator as a good one, bump reputation.
async fn note_good_collation(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	peer_data: &HashMap<PeerId, PeerData>,
	id: CollatorId,
) {
	if let Some(peer_id) = collator_peer_id(peer_data, &id) {
		modify_reputation(sender, peer_id, BENEFIT_NOTIFY_GOOD).await;
	}
}

/// Notify a collator that its collation got seconded.
async fn notify_collation_seconded(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	peer_id: PeerId,
	relay_parent: Hash,
	statement: SignedFullStatement,
) {
	let wire_message =
		protocol_v1::CollatorProtocolMessage::CollationSeconded(relay_parent, statement.into());
	sender
		.send_message(NetworkBridgeTxMessage::SendCollationMessage(
			vec![peer_id],
			Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(wire_message)),
		))
		.await;

	modify_reputation(sender, peer_id, BENEFIT_NOTIFY_GOOD).await;
}

/// A peer's view has changed. A number of things should be done:
///  - Ongoing collation requests have to be canceled.
///  - Advertisements by this peer that are no longer relevant have to be removed.
async fn handle_peer_view_change(state: &mut State, peer_id: PeerId, view: View) -> Result<()> {
	let peer_data = state.peer_data.entry(peer_id).or_default();

	peer_data.update_view(view);
	state
		.requested_collations
		.retain(|pc, _| pc.peer_id != peer_id || peer_data.has_advertised(&pc.relay_parent));

	Ok(())
}

/// Request a collation from the network.
/// This function will
///  - Check for duplicate requests.
///  - Check if the requested collation is in our view.
///  - Update `PerRequest` records with the `result` field if necessary.
/// And as such invocations of this function may rely on that.
async fn request_collation(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	state: &mut State,
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	result: oneshot::Sender<(CandidateReceipt, PoV)>,
) {
	if !state.view.contains(&relay_parent) {
		gum::debug!(
			target: LOG_TARGET,
			peer_id = %peer_id,
			para_id = %para_id,
			relay_parent = %relay_parent,
			"collation is no longer in view",
		);
		return
	}
	let pending_collation = PendingCollation::new(relay_parent, &para_id, &peer_id);
	if state.requested_collations.contains_key(&pending_collation) {
		gum::warn!(
			target: LOG_TARGET,
			peer_id = %pending_collation.peer_id,
			%pending_collation.para_id,
			?pending_collation.relay_parent,
			"collation has already been requested",
		);
		return
	}

	let (full_request, response_recv) = OutgoingRequest::new(
		Recipient::Peer(peer_id),
		CollationFetchingRequest { relay_parent, para_id },
	);
	let requests = Requests::CollationFetchingV1(full_request);

	let per_request = PerRequest {
		from_collator: response_recv.boxed().fuse(),
		to_requester: result,
		span: state
			.span_per_relay_parent
			.get(&relay_parent)
			.map(|s| s.child("collation-request").with_para_id(para_id)),
		_lifetime_timer: state.metrics.time_collation_request_duration(),
	};

	state
		.requested_collations
		.insert(PendingCollation::new(relay_parent, &para_id, &peer_id), per_request);

	gum::debug!(
		target: LOG_TARGET,
		peer_id = %peer_id,
		%para_id,
		?relay_parent,
		"Requesting collation",
	);

	sender
		.send_message(NetworkBridgeTxMessage::SendRequests(
			vec![requests],
			IfDisconnected::ImmediateError,
		))
		.await;
}

/// Networking message has been received.
#[overseer::contextbounds(CollatorProtocol, prefix = overseer)]
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
) {
	use protocol_v1::CollatorProtocolMessage::*;
	use sp_runtime::traits::AppVerify;
	match msg {
		Declare(collator_id, para_id, signature) => {
			if collator_peer_id(&state.peer_data, &collator_id).is_some() {
				modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			let peer_data = match state.peer_data.get_mut(&origin) {
				Some(p) => p,
				None => {
					gum::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						?para_id,
						"Unknown peer",
					);
					modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
					return
				},
			};

			if peer_data.is_collating() {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?para_id,
					"Peer is not in the collating state",
				);
				modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			if !signature.verify(&*protocol_v1::declare_signature_payload(&origin), &collator_id) {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?para_id,
					"Signature verification failure",
				);
				modify_reputation(ctx.sender(), origin, COST_INVALID_SIGNATURE).await;
				return
			}

			if state.active_paras.is_current(&para_id) {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?collator_id,
					?para_id,
					"Declared as collator for current para",
				);

				peer_data.set_collating(collator_id, para_id);
			} else {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?collator_id,
					?para_id,
					"Declared as collator for unneeded para",
				);

				modify_reputation(ctx.sender(), origin, COST_UNNEEDED_COLLATOR).await;
				gum::trace!(target: LOG_TARGET, "Disconnecting unneeded collator");
				disconnect_peer(ctx.sender(), origin).await;
			}
		},
		AdvertiseCollation(relay_parent) => {
			let _span = state
				.span_per_relay_parent
				.get(&relay_parent)
				.map(|s| s.child("advertise-collation"));
			if !state.view.contains(&relay_parent) {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?relay_parent,
					"Advertise collation out of view",
				);

				modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			let peer_data = match state.peer_data.get_mut(&origin) {
				None => {
					gum::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						?relay_parent,
						"Advertise collation message has been received from an unknown peer",
					);
					modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
					return
				},
				Some(p) => p,
			};

			match peer_data.insert_advertisement(relay_parent, &state.view) {
				Ok((id, para_id)) => {
					gum::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						%para_id,
						?relay_parent,
						"Received advertise collation",
					);

					let pending_collation = PendingCollation::new(relay_parent, &para_id, &origin);

					let collations =
						state.collations_per_relay_parent.entry(relay_parent).or_default();

					match collations.status {
						CollationStatus::Fetching | CollationStatus::WaitingOnValidation => {
							gum::trace!(
								target: LOG_TARGET,
								peer_id = ?origin,
								%para_id,
								?relay_parent,
								"Added collation to the pending list"
							);
							collations.unfetched_collations.push((pending_collation, id));
						},
						CollationStatus::Waiting => {
							collations.status = CollationStatus::Fetching;
							collations.waiting_collation = Some(id.clone());

							fetch_collation(ctx.sender(), state, pending_collation.clone(), id)
								.await;
						},
						CollationStatus::Seconded => {
							gum::trace!(
								target: LOG_TARGET,
								peer_id = ?origin,
								%para_id,
								?relay_parent,
								"Valid seconded collation"
							);
						},
					}
				},
				Err(error) => {
					gum::debug!(
						target: LOG_TARGET,
						peer_id = ?origin,
						?relay_parent,
						?error,
						"Invalid advertisement",
					);

					modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
				},
			}
		},
		CollationSeconded(_, _) => {
			gum::warn!(
				target: LOG_TARGET,
				peer_id = ?origin,
				"Unexpected `CollationSeconded` message, decreasing reputation",
			);
		},
	}
}

/// A leaf has become inactive so we want to
///   - Cancel all ongoing collation requests that are on top of that leaf.
///   - Remove all stored collations relevant to that leaf.
async fn remove_relay_parent(state: &mut State, relay_parent: Hash) -> Result<()> {
	state.requested_collations.retain(|k, _| k.relay_parent != relay_parent);

	state.pending_candidates.retain(|k, _| k != &relay_parent);

	state.collations_per_relay_parent.remove(&relay_parent);
	Ok(())
}

/// Our view has changed.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_our_view_change<Context>(
	ctx: &mut Context,
	state: &mut State,
	keystore: &SyncCryptoStorePtr,
	view: OurView,
) -> Result<()> {
	let old_view = std::mem::replace(&mut state.view, view);

	let added: HashMap<Hash, Arc<jaeger::Span>> = state
		.view
		.span_per_head()
		.iter()
		.filter(|v| !old_view.contains(&v.0))
		.map(|v| (*v.0, v.1.clone()))
		.collect();

	added.into_iter().for_each(|(h, s)| {
		state.span_per_relay_parent.insert(h, PerLeafSpan::new(s, "validator-side"));
	});

	let added = state.view.difference(&old_view).cloned().collect::<Vec<_>>();
	let removed = old_view.difference(&state.view).cloned().collect::<Vec<_>>();

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
			if !state.active_paras.is_current(&para_id) {
				gum::trace!(
					target: LOG_TARGET,
					?peer_id,
					?para_id,
					"Disconnecting peer on view change (not current parachain id)"
				);
				disconnect_peer(ctx.sender(), *peer_id).await;
			}
		}
	}

	Ok(())
}

/// Bridge event switch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut State,
	keystore: &SyncCryptoStorePtr,
	bridge_message: NetworkBridgeEvent<net_protocol::CollatorProtocolMessage>,
) -> Result<()> {
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(peer_id, _role, _version, _) => {
			state.peer_data.entry(peer_id).or_default();
			state.metrics.note_collator_peer_count(state.peer_data.len());
		},
		PeerDisconnected(peer_id) => {
			state.peer_data.remove(&peer_id);
			state.metrics.note_collator_peer_count(state.peer_data.len());
		},
		NewGossipTopology { .. } => {
			// impossible!
		},
		PeerViewChange(peer_id, view) => {
			handle_peer_view_change(state, peer_id, view).await?;
		},
		OurViewChange(view) => {
			handle_our_view_change(ctx, state, keystore, view).await?;
		},
		PeerMessage(remote, Versioned::V1(msg)) => {
			process_incoming_peer_message(ctx, state, remote, msg).await;
		},
	}

	Ok(())
}

/// The main message receiver switch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn process_msg<Context>(
	ctx: &mut Context,
	keystore: &SyncCryptoStorePtr,
	msg: CollatorProtocolMessage,
	state: &mut State,
) {
	use CollatorProtocolMessage::*;

	let _timer = state.metrics.time_process_msg();

	match msg {
		CollateOn(id) => {
			gum::warn!(
				target: LOG_TARGET,
				para_id = %id,
				"CollateOn message is not expected on the validator side of the protocol",
			);
		},
		DistributeCollation(_, _, _) => {
			gum::warn!(
				target: LOG_TARGET,
				"DistributeCollation message is not expected on the validator side of the protocol",
			);
		},
		ReportCollator(id) => {
			report_collator(ctx.sender(), &state.peer_data, id).await;
		},
		NetworkBridgeUpdate(event) => {
			if let Err(e) = handle_network_msg(ctx, state, keystore, event).await {
				gum::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to handle incoming network message",
				);
			}
		},
		Seconded(parent, stmt) => {
			if let Some(collation_event) = state.pending_candidates.remove(&parent) {
				let (collator_id, pending_collation) = collation_event;
				let PendingCollation { relay_parent, peer_id, .. } = pending_collation;
				note_good_collation(ctx.sender(), &state.peer_data, collator_id).await;
				notify_collation_seconded(ctx.sender(), peer_id, relay_parent, stmt).await;

				if let Some(collations) = state.collations_per_relay_parent.get_mut(&parent) {
					collations.status = CollationStatus::Seconded;
				}
			} else {
				gum::debug!(
					target: LOG_TARGET,
					relay_parent = ?parent,
					"Collation has been seconded, but the relay parent is deactivated",
				);
			}
		},
		Invalid(parent, candidate_receipt) => {
			let id = match state.pending_candidates.entry(parent) {
				Entry::Occupied(entry)
					if entry.get().1.commitments_hash ==
						Some(candidate_receipt.commitments_hash) =>
					entry.remove().0,
				Entry::Occupied(_) => {
					gum::error!(
						target: LOG_TARGET,
						relay_parent = ?parent,
						candidate = ?candidate_receipt.hash(),
						"Reported invalid candidate for unknown `pending_candidate`!",
					);
					return
				},
				Entry::Vacant(_) => return,
			};

			report_collator(ctx.sender(), &state.peer_data, id.clone()).await;

			dequeue_next_collation_and_fetch(ctx, state, parent, id).await;
		},
	}
}

/// The main run loop.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
pub(crate) async fn run<Context>(
	mut ctx: Context,
	keystore: SyncCryptoStorePtr,
	eviction_policy: crate::CollatorEvictionPolicy,
	metrics: Metrics,
) -> std::result::Result<(), crate::error::FatalError> {
	let mut state = State { metrics, ..Default::default() };

	let next_inactivity_stream = tick_stream(ACTIVITY_POLL);
	futures::pin_mut!(next_inactivity_stream);

	let check_collations_stream = tick_stream(CHECK_COLLATIONS_POLL);
	futures::pin_mut!(check_collations_stream);

	loop {
		select! {
			res = ctx.recv().fuse() => {
				match res {
					Ok(FromOrchestra::Communication { msg }) => {
						gum::trace!(target: LOG_TARGET, msg = ?msg, "received a message");
						process_msg(
							&mut ctx,
							&keystore,
							msg,
							&mut state,
						).await;
					}
					Ok(FromOrchestra::Signal(OverseerSignal::Conclude)) | Err(_) => break,
					Ok(FromOrchestra::Signal(_)) => continue,
				}
			}
			_ = next_inactivity_stream.next() => {
				disconnect_inactive_peers(ctx.sender(), &eviction_policy, &state.peer_data).await;
			}
			res = state.collation_fetches.select_next_some() => {
				handle_collation_fetched_result(&mut ctx, &mut state, res).await;
			}
			res = state.collation_fetch_timeouts.select_next_some() => {
				let (collator_id, relay_parent) = res;
				gum::debug!(
					target: LOG_TARGET,
					?relay_parent,
					?collator_id,
					"Timeout hit - already seconded?"
				);
				dequeue_next_collation_and_fetch(&mut ctx, &mut state, relay_parent, collator_id).await;
			}
			_ = check_collations_stream.next() => {
				let reputation_changes = poll_requests(
					&mut state.requested_collations,
					&state.metrics,
					&state.span_per_relay_parent,
				).await;

				for (peer_id, rep) in reputation_changes {
					modify_reputation(ctx.sender(), peer_id, rep).await;
				}
			},
		}
	}

	Ok(())
}

async fn poll_requests(
	requested_collations: &mut HashMap<PendingCollation, PerRequest>,
	metrics: &Metrics,
	span_per_relay_parent: &HashMap<Hash, PerLeafSpan>,
) -> Vec<(PeerId, Rep)> {
	let mut retained_requested = HashSet::new();
	let mut reputation_changes = Vec::new();
	for (pending_collation, per_req) in requested_collations.iter_mut() {
		// Despite the await, this won't block on the response itself.
		let result =
			poll_collation_response(metrics, span_per_relay_parent, pending_collation, per_req)
				.await;

		if !result.is_ready() {
			retained_requested.insert(pending_collation.clone());
		}
		if let CollationFetchResult::Error(Some(rep)) = result {
			reputation_changes.push((pending_collation.peer_id, rep));
		}
	}
	requested_collations.retain(|k, _| retained_requested.contains(k));
	reputation_changes
}

/// Dequeue another collation and fetch.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn dequeue_next_collation_and_fetch<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	// The collator we tried to fetch from last.
	previous_fetch: CollatorId,
) {
	if let Some((next, id)) = state
		.collations_per_relay_parent
		.get_mut(&relay_parent)
		.and_then(|c| c.get_next_collation_to_fetch(Some(&previous_fetch)))
	{
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			?id,
			"Successfully dequeued next advertisement - fetching ..."
		);
		fetch_collation(ctx.sender(), state, next, id).await;
	} else {
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			previous_collator = ?previous_fetch,
			"No collations are available to fetch"
		);
	}
}

/// Handle a fetched collation result.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn handle_collation_fetched_result<Context>(
	ctx: &mut Context,
	state: &mut State,
	(mut collation_event, res): PendingCollationFetch,
) {
	// If no prior collation for this relay parent has been seconded, then
	// memorize the `collation_event` for that `relay_parent`, such that we may
	// notify the collator of their successful second backing
	let relay_parent = collation_event.1.relay_parent;

	let (candidate_receipt, pov) = match res {
		Ok(res) => res,
		Err(e) => {
			gum::debug!(
				target: LOG_TARGET,
				relay_parent = ?collation_event.1.relay_parent,
				para_id = ?collation_event.1.para_id,
				peer_id = ?collation_event.1.peer_id,
				collator_id = ?collation_event.0,
				error = ?e,
				"Failed to fetch collation.",
			);

			dequeue_next_collation_and_fetch(ctx, state, relay_parent, collation_event.0).await;
			return
		},
	};

	if let Some(collations) = state.collations_per_relay_parent.get_mut(&relay_parent) {
		if let CollationStatus::Seconded = collations.status {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				"Already seconded - no longer interested in collation fetch result."
			);
			return
		}
		collations.status = CollationStatus::WaitingOnValidation;
	}

	if let Entry::Vacant(entry) = state.pending_candidates.entry(relay_parent) {
		collation_event.1.commitments_hash = Some(candidate_receipt.commitments_hash);
		ctx.sender()
			.send_message(CandidateBackingMessage::Second(relay_parent, candidate_receipt, pov))
			.await;

		entry.insert(collation_event);
	} else {
		gum::trace!(
			target: LOG_TARGET,
			?relay_parent,
			candidate = ?candidate_receipt.hash(),
			"Trying to insert a pending candidate failed, because there is already one.",
		)
	}
}

// This issues `NetworkBridge` notifications to disconnect from all inactive peers at the
// earliest possible point. This does not yet clean up any metadata, as that will be done upon
// receipt of the `PeerDisconnected` event.
async fn disconnect_inactive_peers(
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	eviction_policy: &crate::CollatorEvictionPolicy,
	peers: &HashMap<PeerId, PeerData>,
) {
	for (peer, peer_data) in peers {
		if peer_data.is_inactive(&eviction_policy) {
			gum::trace!(target: LOG_TARGET, "Disconnecting inactive peer");
			disconnect_peer(sender, *peer).await;
		}
	}
}

enum CollationFetchResult {
	/// The collation is still being fetched.
	Pending,
	/// The collation was fetched successfully.
	Success,
	/// An error occurred when fetching a collation or it was invalid.
	/// A given reputation change should be applied to the peer.
	Error(Option<Rep>),
}

impl CollationFetchResult {
	fn is_ready(&self) -> bool {
		!matches!(self, Self::Pending)
	}
}

/// Poll collation response, return immediately if there is none.
///
/// Ready responses are handled, by logging and by
/// forwarding proper responses to the requester.
async fn poll_collation_response(
	metrics: &Metrics,
	spans: &HashMap<Hash, PerLeafSpan>,
	pending_collation: &PendingCollation,
	per_req: &mut PerRequest,
) -> CollationFetchResult {
	if never!(per_req.from_collator.is_terminated()) {
		gum::error!(
			target: LOG_TARGET,
			"We remove pending responses once received, this should not happen."
		);
		return CollationFetchResult::Success
	}

	if let Poll::Ready(response) = futures::poll!(&mut per_req.from_collator) {
		let _span = spans
			.get(&pending_collation.relay_parent)
			.map(|s| s.child("received-collation"));
		let _timer = metrics.time_handle_collation_request_result();

		let mut metrics_result = Err(());
		let mut success = "false";

		let result = match response {
			Err(RequestError::InvalidResponse(err)) => {
				gum::warn!(
					target: LOG_TARGET,
					hash = ?pending_collation.relay_parent,
					para_id = ?pending_collation.para_id,
					peer_id = ?pending_collation.peer_id,
					err = ?err,
					"Collator provided response that could not be decoded"
				);
				CollationFetchResult::Error(Some(COST_CORRUPTED_MESSAGE))
			},
			Err(err) if err.is_timed_out() => {
				gum::debug!(
					target: LOG_TARGET,
					hash = ?pending_collation.relay_parent,
					para_id = ?pending_collation.para_id,
					peer_id = ?pending_collation.peer_id,
					"Request timed out"
				);
				// For now we don't want to change reputation on timeout, to mitigate issues like
				// this: https://github.com/paritytech/polkadot/issues/4617
				CollationFetchResult::Error(None)
			},
			Err(RequestError::NetworkError(err)) => {
				gum::debug!(
					target: LOG_TARGET,
					hash = ?pending_collation.relay_parent,
					para_id = ?pending_collation.para_id,
					peer_id = ?pending_collation.peer_id,
					err = ?err,
					"Fetching collation failed due to network error"
				);
				// A minor decrease in reputation for any network failure seems
				// sensible. In theory this could be exploited, by DoSing this node,
				// which would result in reduced reputation for proper nodes, but the
				// same can happen for penalties on timeouts, which we also have.
				CollationFetchResult::Error(Some(COST_NETWORK_ERROR))
			},
			Err(RequestError::Canceled(err)) => {
				gum::debug!(
					target: LOG_TARGET,
					hash = ?pending_collation.relay_parent,
					para_id = ?pending_collation.para_id,
					peer_id = ?pending_collation.peer_id,
					err = ?err,
					"Canceled should be handled by `is_timed_out` above - this is a bug!"
				);
				CollationFetchResult::Error(None)
			},
			Ok(CollationFetchingResponse::Collation(receipt, _))
				if receipt.descriptor().para_id != pending_collation.para_id =>
			{
				gum::debug!(
					target: LOG_TARGET,
					expected_para_id = ?pending_collation.para_id,
					got_para_id = ?receipt.descriptor().para_id,
					peer_id = ?pending_collation.peer_id,
					"Got wrong para ID for requested collation."
				);

				CollationFetchResult::Error(Some(COST_WRONG_PARA))
			},
			Ok(CollationFetchingResponse::Collation(receipt, pov)) => {
				gum::debug!(
					target: LOG_TARGET,
					para_id = %pending_collation.para_id,
					hash = ?pending_collation.relay_parent,
					candidate_hash = ?receipt.hash(),
					"Received collation",
				);
				// Actual sending:
				let _span = jaeger::Span::new(&pov, "received-collation");
				let (mut tx, _) = oneshot::channel();
				std::mem::swap(&mut tx, &mut (per_req.to_requester));
				let result = tx.send((receipt, pov));

				if let Err(_) = result {
					gum::warn!(
						target: LOG_TARGET,
						hash = ?pending_collation.relay_parent,
						para_id = ?pending_collation.para_id,
						peer_id = ?pending_collation.peer_id,
						"Sending response back to requester failed (receiving side closed)"
					);
				} else {
					metrics_result = Ok(());
					success = "true";
				}

				CollationFetchResult::Success
			},
		};
		metrics.on_request(metrics_result);
		per_req.span.as_mut().map(|s| s.add_string_tag("success", success));

		result
	} else {
		CollationFetchResult::Pending
	}
}
