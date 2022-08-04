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
	stream::{FusedStream, FuturesUnordered},
	FutureExt, StreamExt,
};
use futures_timer::Delay;
use std::{
	collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
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
		v1 as request_v1, vstaging as request_vstaging, OutgoingRequest, Requests,
	},
	v1 as protocol_v1, vstaging as protocol_vstaging, OurView, PeerId,
	UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::{PoV, SignedFullStatement, Statement};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		CandidateBackingMessage, CollatorProtocolMessage, IfDisconnected, NetworkBridgeEvent,
		NetworkBridgeMessage,
	},
	overseer, CollatorProtocolSenderTrait, FromOrchestra, OverseerSignal, PerLeafSpan,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::View as ImplicitView, metrics::prometheus::prometheus::HistogramTimer,
};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, CollatorId, Hash, Id as ParaId, OccupiedCoreAssumption,
	PersistedValidationData,
};

use crate::error::{Error, Result};

use super::{
	modify_reputation, prospective_parachains_mode, ProspectiveParachainsMode, LOG_TARGET,
	MAX_CANDIDATE_DEPTH,
};

mod metrics;

#[cfg(test)]
mod tests;

pub use metrics::Metrics;

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
const MAX_UNSHARED_DOWNLOAD_TIME: Duration = Duration::from_millis(400);

// How often to check all peers with activity.
#[cfg(not(test))]
const ACTIVITY_POLL: Duration = Duration::from_secs(1);

#[cfg(test)]
const ACTIVITY_POLL: Duration = Duration::from_millis(10);

// How often to poll collation responses.
// This is a hack that should be removed in a refactoring.
// See https://github.com/paritytech/polkadot/issues/4182
const CHECK_COLLATIONS_POLL: Duration = Duration::from_millis(5);

struct PerRequest {
	/// Responses from collator.
	from_collator:
		Fuse<BoxFuture<'static, req_res::OutgoingResult<request_v1::CollationFetchingResponse>>>,
	/// Sender to forward to initial requester.
	to_requester: oneshot::Sender<(CandidateReceipt, PoV)>,
	/// A jaeger span corresponding to the lifetime of the request.
	span: Option<jaeger::Span>,
	/// A metric histogram for the lifetime of the request
	_lifetime_timer: Option<HistogramTimer>,
}

#[derive(Debug)]
struct CollatingPeerState {
	collator_id: CollatorId,
	para_id: ParaId,
	// Advertised relay parents.
	advertisements: HashMap<Hash, HashSet<CandidateHash>>,
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
	/// Advertisement is already known.
	Duplicate,
	/// Collation relay parent is out of our view.
	OutOfOurView,
	/// No prior declare message received.
	UndeclaredCollator,
	/// A limit for announcements per peer is reached.
	LimitReached,
	/// Mismatch of relay parent mode and advertisement arguments.
	/// An internal error that should not happen.
	InvalidArguments,
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
	fn update_view(
		&mut self,
		implicit_view: &ImplicitView,
		active_leaves: &HashMap<Hash, ProspectiveParachainsMode>,
		per_relay_parent: &HashMap<Hash, PerRelayParent>,
		new_view: View,
	) {
		let old_view = std::mem::replace(&mut self.view, new_view);
		if let PeerState::Collating(ref mut peer_state) = self.state {
			for removed in old_view.difference(&self.view) {
				// Only keep advertisements if prospective parachains
				// are enabled and the relay parent is a part of allowed
				// ancestry.
				let relay_parent_mode_enabled = per_relay_parent
					.get(removed)
					.map_or(false, |s| s.prospective_parachains_mode.is_enabled());
				let keep = relay_parent_mode_enabled &&
					is_relay_parent_in_view(
						removed,
						ProspectiveParachainsMode::Enabled,
						implicit_view,
						active_leaves,
						peer_state.para_id,
					);

				if !keep {
					peer_state.advertisements.remove(&removed);
				}
			}
		}
	}

	/// Prune old advertisements relative to our view.
	fn prune_old_advertisements(
		&mut self,
		implicit_view: &ImplicitView,
		active_leaves: &HashMap<Hash, ProspectiveParachainsMode>,
		per_relay_parent: &HashMap<Hash, PerRelayParent>,
	) {
		if let PeerState::Collating(ref mut peer_state) = self.state {
			peer_state.advertisements.retain(|hash, _| {
				// Either
				// - Relay parent is an active leaf
				// - It belongs to allowed ancestry under some leaf
				// Discard otherwise.
				per_relay_parent.get(hash).map_or(false, |s| {
					is_relay_parent_in_view(
						hash,
						s.prospective_parachains_mode,
						implicit_view,
						active_leaves,
						peer_state.para_id,
					)
				})
			});
		}
	}

	/// Note an advertisement by the collator. Returns `true` if the advertisement was imported
	/// successfully. Fails if the advertisement is duplicate, out of view, or the peer has not
	/// declared itself a collator.
	fn insert_advertisement(
		&mut self,
		on_relay_parent: Hash,
		relay_parent_mode: ProspectiveParachainsMode,
		candidate_hash: Option<CandidateHash>,
		implicit_view: &ImplicitView,
		active_leaves: &HashMap<Hash, ProspectiveParachainsMode>,
	) -> std::result::Result<(CollatorId, ParaId), AdvertisementError> {
		match self.state {
			PeerState::Connected(_) => Err(AdvertisementError::UndeclaredCollator),
			PeerState::Collating(ref mut state) => {
				if !is_relay_parent_in_view(
					&on_relay_parent,
					relay_parent_mode,
					implicit_view,
					active_leaves,
					state.para_id,
				) {
					return Err(AdvertisementError::OutOfOurView)
				}

				match (relay_parent_mode, candidate_hash) {
					(ProspectiveParachainsMode::Disabled, None) => {
						if state.advertisements.contains_key(&on_relay_parent) {
							return Err(AdvertisementError::Duplicate)
						}
						state.advertisements.insert(on_relay_parent, HashSet::new());
					},
					(ProspectiveParachainsMode::Enabled, Some(candidate_hash)) => {
						if state
							.advertisements
							.get(&on_relay_parent)
							.map_or(false, |candidates| candidates.contains(&candidate_hash))
						{
							return Err(AdvertisementError::Duplicate)
						}
						let candidates = state.advertisements.entry(on_relay_parent).or_default();

						if candidates.len() >= MAX_CANDIDATE_DEPTH + 1 {
							return Err(AdvertisementError::LimitReached)
						}
						candidates.insert(candidate_hash);
					},
					_ => return Err(AdvertisementError::InvalidArguments),
				}

				state.last_active = Instant::now();
				Ok((state.collator_id, state.para_id))
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
			advertisements: HashMap::new(),
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
	fn has_advertised(
		&self,
		relay_parent: &Hash,
		maybe_candidate_hash: Option<CandidateHash>,
	) -> bool {
		let collating_state = match self.state {
			PeerState::Connected(_) => return false,
			PeerState::Collating(ref state) => state,
		};

		if let Some(ref candidate_hash) = maybe_candidate_hash {
			collating_state
				.advertisements
				.get(relay_parent)
				.map_or(false, |candidates| candidates.contains(candidate_hash))
		} else {
			collating_state.advertisements.contains_key(relay_parent)
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

/// Identifier of a fetched collation.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct FetchedCollation {
	relay_parent: Hash,
	para_id: ParaId,
	candidate_hash: CandidateHash,
	collator_id: CollatorId,
}

impl From<&CandidateReceipt<Hash>> for FetchedCollation {
	fn from(receipt: &CandidateReceipt<Hash>) -> Self {
		let descriptor = receipt.descriptor();
		Self {
			relay_parent: descriptor.relay_parent,
			para_id: descriptor.para_id,
			candidate_hash: receipt.hash(),
			collator_id: descriptor.collator.clone(),
		}
	}
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct PendingCollation {
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	candidate_hash: Option<CandidateHash>,
	commitments_hash: Option<Hash>,
}

impl PendingCollation {
	fn new(
		relay_parent: Hash,
		para_id: &ParaId,
		peer_id: &PeerId,
		candidate_hash: Option<CandidateHash>,
	) -> Self {
		Self {
			relay_parent,
			para_id: para_id.clone(),
			peer_id: peer_id.clone(),
			candidate_hash,
			commitments_hash: None,
		}
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
	fn back_to_waiting(&mut self, relay_parent_mode: ProspectiveParachainsMode) {
		match self {
			Self::Seconded =>
				if relay_parent_mode.is_enabled() {
					// With async backing enabled it's allowed to
					// second more candidates.
					*self = Self::Waiting
				},
			_ => *self = Self::Waiting,
		}
	}
}

/// Information about collations per relay parent.
#[derive(Default)]
struct Collations {
	/// What is the current status in regards to a collation for this relay parent?
	status: CollationStatus,
	/// Collator we're fetching from.
	///
	/// This is the currently last started fetch, which did not exceed `MAX_UNSHARED_DOWNLOAD_TIME`
	/// yet.
	fetching_from: Option<CollatorId>,
	/// Collation that were advertised to us, but we did not yet fetch.
	waiting_queue: VecDeque<(PendingCollation, CollatorId)>,
	/// How many collations have been seconded per parachain.
	/// Only used when async backing is enabled.
	seconded_count: HashMap<ParaId, usize>,
}

impl Collations {
	/// Returns the next collation to fetch from the `unfetched_collations`.
	///
	/// This will reset the status back to `Waiting` using [`CollationStatus::back_to_waiting`].
	///
	/// Returns `Some(_)` if there is any collation to fetch, the `status` is not `Seconded` and
	/// the passed in `finished_one` is the currently `waiting_collation`.
	fn get_next_collation_to_fetch(
		&mut self,
		finished_one: Option<&CollatorId>,
		relay_parent_mode: ProspectiveParachainsMode,
	) -> Option<(PendingCollation, CollatorId)> {
		// If finished one does not match waiting_collation, then we already dequeued another fetch
		// to replace it.
		if self.fetching_from.as_ref() != finished_one {
			gum::trace!(
				target: LOG_TARGET,
				waiting_collation = ?self.fetching_from,
				?finished_one,
				"Not proceeding to the next collation - has already been done."
			);
			return None
		}
		self.status.back_to_waiting(relay_parent_mode);

		match self.status {
			// We don't need to fetch any other collation when we already have seconded one.
			CollationStatus::Seconded => None,
			CollationStatus::Waiting => {
				while let Some(next) = self.waiting_queue.pop_front() {
					let para_id = next.0.para_id;
					if !self.is_fetch_allowed(relay_parent_mode, para_id) {
						continue
					}

					return Some(next)
				}

				None
			},
			CollationStatus::WaitingOnValidation | CollationStatus::Fetching =>
				unreachable!("We have reset the status above!"),
		}
	}

	/// Checks the limit of seconded candidates for a given para.
	fn is_fetch_allowed(
		&self,
		relay_parent_mode: ProspectiveParachainsMode,
		para_id: ParaId,
	) -> bool {
		let seconded_limit =
			if relay_parent_mode.is_enabled() { MAX_CANDIDATE_DEPTH + 1 } else { 1 };
		self.seconded_count.get(&para_id).map_or(true, |&num| num < seconded_limit)
	}
}

#[derive(Debug, Copy, Clone)]
struct GroupAssignments {
	current: Option<ParaId>,
}

struct PerRelayParent {
	prospective_parachains_mode: ProspectiveParachainsMode,
	assignment: GroupAssignments,
	collations: Collations,
}

impl PerRelayParent {
	fn new(mode: ProspectiveParachainsMode) -> Self {
		Self {
			prospective_parachains_mode: mode,
			assignment: GroupAssignments { current: None },
			collations: Collations::default(),
		}
	}
}

/// All state relevant for the validator side of the protocol lives here.
#[derive(Default)]
struct State {
	/// Leaves that do support asynchronous backing along with
	/// implicit ancestry. Leaves from the implicit view are present in
	/// `active_leaves`, the opposite doesn't hold true.
	///
	/// Relay-chain blocks which don't support prospective parachains are
	/// never included in the fragment trees of active leaves which do. In
	/// particular, this means that if a given relay parent belongs to implicit
	/// ancestry of some active leaf, then it does support prospective parachains.
	implicit_view: ImplicitView,

	/// All active leaves observed by us, including both that do and do not
	/// support prospective parachains. This mapping works as a replacement for
	/// [`polkadot_node_network_protocol::View`] and can be dropped once the transition
	/// to asynchronous backing is done.
	active_leaves: HashMap<Hash, ProspectiveParachainsMode>,

	/// State tracked
	per_relay_parent: HashMap<Hash, PerRelayParent>,

	/// Track all active collators and their data.
	peer_data: HashMap<PeerId, PeerData>,

	/// Parachains we're currently assigned to. With async backing enabled
	/// this includes assignments from the implicit view.
	current_assignments: HashMap<ParaId, usize>,

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

	/// Keep track of all pending candidate collations
	fetched_candidates: HashMap<FetchedCollation, CollationEvent>,
}

fn is_relay_parent_in_view(
	relay_parent: &Hash,
	relay_parent_mode: ProspectiveParachainsMode,
	implicit_view: &ImplicitView,
	active_leaves: &HashMap<Hash, ProspectiveParachainsMode>,
	para_id: ParaId,
) -> bool {
	match relay_parent_mode {
		ProspectiveParachainsMode::Disabled => true,
		ProspectiveParachainsMode::Enabled => active_leaves.iter().any(|(hash, mode)| {
			mode.is_enabled() &&
				implicit_view
					.known_allowed_relay_parents_under(hash, Some(para_id))
					.unwrap_or_default()
					.contains(relay_parent)
		}),
	}
}

async fn assign_incoming<Sender>(
	sender: &mut Sender,
	group_assignment: &mut GroupAssignments,
	current_assignments: &mut HashMap<ParaId, usize>,
	keystore: &SyncCryptoStorePtr,
	relay_parent: Hash,
) where
	Sender: CollatorProtocolSenderTrait,
{
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
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				"Failed to query runtime API for relay-parent",
			);

			return
		},
	};

	let para_now = match polkadot_node_subsystem_util::signing_key_and_index(&validators, keystore)
		.await
		.and_then(|(_, index)| polkadot_node_subsystem_util::find_validator_group(&groups, index))
	{
		Some(group) => {
			let core_now = rotation_info.core_for_group(group, cores.len());

			cores.get(core_now.0 as usize).and_then(|c| c.para_id())
		},
		None => {
			gum::trace!(target: LOG_TARGET, ?relay_parent, "Not a validator");

			return
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
		let entry = current_assignments.entry(para_now).or_default();
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

	*group_assignment = GroupAssignments { current: para_now };
}

fn remove_outgoing(
	current_assignments: &mut HashMap<ParaId, usize>,
	per_relay_parent: PerRelayParent,
) {
	let GroupAssignments { current } = per_relay_parent.assignment;

	if let Some(cur) = current {
		if let Entry::Occupied(mut occupied) = current_assignments.entry(cur) {
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

// O(n) search for collator ID by iterating through the peers map. This should be fast enough
// unless a large amount of peers is expected.
fn collator_peer_id(
	peer_data: &HashMap<PeerId, PeerData>,
	collator_id: &CollatorId,
) -> Option<PeerId> {
	peer_data.iter().find_map(|(peer, data)| {
		data.collator_id().filter(|c| c == &collator_id).map(|_| peer.clone())
	})
}

async fn disconnect_peer(sender: &mut impl overseer::CollatorProtocolSenderTrait, peer_id: PeerId) {
	sender
		.send_message(NetworkBridgeMessage::DisconnectPeer(peer_id, PeerSet::Collation))
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

	let PendingCollation { relay_parent, para_id, peer_id, candidate_hash, .. } = pc;

	if let Some(peer_data) = state.peer_data.get(&peer_id) {
		// If candidate hash is `Some` then relay parent supports prospective
		// parachains.
		if peer_data.has_advertised(&relay_parent, candidate_hash) {
			let timeout = |collator_id, relay_parent| async move {
				Delay::new(MAX_UNSHARED_DOWNLOAD_TIME).await;
				(collator_id, relay_parent)
			};
			state
				.collation_fetch_timeouts
				.push(timeout(id.clone(), relay_parent.clone()).boxed());
			request_collation(
				sender,
				state,
				relay_parent,
				para_id,
				candidate_hash,
				peer_id,
				id.clone(),
				tx,
			)
			.await;

			state.collation_fetches.push(rx.map(|r| ((id, pc), r)).boxed());
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
		.send_message(NetworkBridgeMessage::SendCollationMessage(
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
	let peer_data = state.peer_data.entry(peer_id.clone()).or_default();

	peer_data.update_view(
		&state.implicit_view,
		&state.active_leaves,
		&state.per_relay_parent,
		view,
	);
	state
		.requested_collations
		.retain(|pc, _| pc.peer_id != peer_id || !peer_data.has_advertised(&pc.relay_parent, None));

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
	candidate_hash: Option<CandidateHash>,
	peer_id: PeerId,
	collator_id: CollatorId,
	result: oneshot::Sender<(CandidateReceipt, PoV)>,
) {
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(state) => state,
		None => {
			gum::debug!(
				target: LOG_TARGET,
				peer_id = %peer_id,
				para_id = %para_id,
				relay_parent = %relay_parent,
				"Collation relay parent is out of view",
			);
			return
		},
	};
	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;
	let pending_collation = PendingCollation::new(relay_parent, &para_id, &peer_id, candidate_hash);
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

	let (requests, response_recv) = match (relay_parent_mode, candidate_hash) {
		(ProspectiveParachainsMode::Disabled, None) => {
			let (req, response_recv) = OutgoingRequest::new(
				Recipient::Peer(peer_id),
				request_v1::CollationFetchingRequest { relay_parent, para_id },
			);
			let requests = Requests::CollationFetchingV1(req);
			(requests, response_recv.boxed())
		},
		(ProspectiveParachainsMode::Enabled, Some(candidate_hash)) => {
			let (req, response_recv) = OutgoingRequest::new(
				Recipient::Peer(peer_id),
				request_vstaging::CollationFetchingRequest {
					relay_parent,
					para_id,
					candidate_hash,
				},
			);
			let requests = Requests::CollationFetchingVStaging(req);
			(requests, response_recv.boxed())
		},
		_ => {
			gum::error!(
				target: LOG_TARGET,
				peer_id = %peer_id,
				%para_id,
				?relay_parent,
				"Invalid arguments for collation request",
			);
			return
		},
	};

	let per_request = PerRequest {
		from_collator: response_recv.fuse(),
		to_requester: result,
		span: state
			.span_per_relay_parent
			.get(&relay_parent)
			.map(|s| s.child("collation-request").with_para_id(para_id)),
		_lifetime_timer: state.metrics.time_collation_request_duration(),
	};

	state.requested_collations.insert(
		PendingCollation::new(relay_parent, &para_id, &peer_id, candidate_hash),
		per_request,
	);

	gum::debug!(
		target: LOG_TARGET,
		peer_id = %peer_id,
		%para_id,
		?relay_parent,
		"Requesting collation",
	);

	per_relay_parent.collations.status = CollationStatus::Fetching;
	per_relay_parent.collations.fetching_from.replace(collator_id);

	sender
		.send_message(NetworkBridgeMessage::SendRequests(
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
	msg: Versioned<
		protocol_v1::CollatorProtocolMessage,
		protocol_vstaging::CollatorProtocolMessage,
	>,
) {
	use protocol_v1::CollatorProtocolMessage as V1;
	use protocol_vstaging::CollatorProtocolMessage as VStaging;
	use sp_runtime::traits::AppVerify;

	match msg {
		Versioned::V1(V1::Declare(collator_id, para_id, signature)) |
		Versioned::VStaging(VStaging::Declare(collator_id, para_id, signature)) => {
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

			if state.current_assignments.contains_key(&para_id) {
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

				modify_reputation(ctx.sender(), origin.clone(), COST_UNNEEDED_COLLATOR).await;
				gum::trace!(target: LOG_TARGET, "Disconnecting unneeded collator");
				disconnect_peer(ctx.sender(), origin).await;
			}
		},
		Versioned::V1(V1::AdvertiseCollation(relay_parent)) =>
			handle_advertisement(ctx.sender(), state, relay_parent, &origin, None).await,
		Versioned::VStaging(VStaging::AdvertiseCollation {
			relay_parent,
			candidate_hash,
			parent_head_data_hash,
		}) =>
			handle_advertisement(
				ctx.sender(),
				state,
				relay_parent,
				&origin,
				Some((candidate_hash, parent_head_data_hash)),
			)
			.await,
		Versioned::V1(V1::CollationSeconded(..)) |
		Versioned::VStaging(VStaging::CollationSeconded(..)) => {
			gum::warn!(
				target: LOG_TARGET,
				peer_id = ?origin,
				"Unexpected `CollationSeconded` message, decreasing reputation",
			);

			modify_reputation(ctx.sender(), origin, COST_UNEXPECTED_MESSAGE).await;
		},
	}
}

// async fn request_hypothetical_depth<Sender>(
// 	sender: &mut Sender,
// 	relay_parent: Hash,
// 	candidate_hash: CandidateHash,
// 	para_id: ParaId,
// ) -> Option<Vec<usize>>
// where
// 	Sender: CollatorProtocolSenderTrait, {
// 		let (tx, rx) = oneshot::channel();

// 		let request = HypotheticalDepthRequest {
// 			candidate_hash,
// 			candidate_para: todo!(),
// 			parent_head_data_hash: todo!(),
// 			candidate_relay_parent: todo!(),
// 			fragment_tree_relay_parent: todo!(),
// 		};
// 	}

async fn handle_advertisement<Sender>(
	sender: &mut Sender,
	state: &mut State,
	relay_parent: Hash,
	peer_id: &PeerId,
	vstaging_args: Option<(CandidateHash, Hash)>,
) where
	Sender: CollatorProtocolSenderTrait,
{
	let _span = state
		.span_per_relay_parent
		.get(&relay_parent)
		.map(|s| s.child("advertise-collation"));
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(state) => state,
		None => {
			gum::debug!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				?relay_parent,
				"Advertise collation out of view",
			);

			modify_reputation(sender, *peer_id, COST_UNEXPECTED_MESSAGE).await;
			return
		},
	};
	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;

	let peer_data = match state.peer_data.get_mut(&peer_id) {
		None => {
			gum::debug!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				?relay_parent,
				"Advertise collation message has been received from an unknown peer",
			);
			modify_reputation(sender, *peer_id, COST_UNEXPECTED_MESSAGE).await;
			return
		},
		Some(p) => p,
	};
	let para_id = if let Some(id) = peer_data.collating_para() {
		id
	} else {
		gum::debug!(
			target: LOG_TARGET,
			peer_id = ?peer_id,
			?relay_parent,
			"Advertise collation message received from undeclared peer",
		);
		modify_reputation(sender, *peer_id, COST_UNEXPECTED_MESSAGE).await;
		return
	};

	let insert_result = match (relay_parent_mode, vstaging_args) {
		(ProspectiveParachainsMode::Disabled, None) => peer_data.insert_advertisement(
			relay_parent,
			relay_parent_mode,
			None,
			&state.implicit_view,
			&state.active_leaves,
		),
		(ProspectiveParachainsMode::Enabled, Some((candidate_hash, parent_head_data_hash))) => {
			// TODO [now]: request hypothetical depth and check for backed parent nodes
			// in a fragment tree.
			peer_data.insert_advertisement(
				relay_parent,
				relay_parent_mode,
				Some(candidate_hash),
				&state.implicit_view,
				&state.active_leaves,
			)
		},
		_ => {
			gum::warn!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				?relay_parent,
				"Invalid arguments for advertisement",
			);
			return
		},
	};

	match insert_result {
		Ok((id, para_id)) => {
			gum::debug!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				%para_id,
				?relay_parent,
				"Received advertise collation",
			);

			let maybe_candidate_hash = vstaging_args.map(|(candidate_hash, _)| candidate_hash);
			let pending_collation =
				PendingCollation::new(relay_parent, &para_id, peer_id, maybe_candidate_hash);

			let collations = &mut per_relay_parent.collations;
			if !collations.is_fetch_allowed(relay_parent_mode, para_id) {
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?peer_id,
					para_id = ?para_id,
					?relay_parent,
					"Seconded collations limit reached",
				);
				return
			}

			match collations.status {
				CollationStatus::Fetching | CollationStatus::WaitingOnValidation => {
					gum::trace!(
						target: LOG_TARGET,
						peer_id = ?peer_id,
						%para_id,
						?relay_parent,
						"Added collation to the pending list"
					);
					collations.waiting_queue.push_back((pending_collation, id));
				},
				CollationStatus::Waiting => {
					fetch_collation(sender, state, pending_collation.clone(), id).await;
				},
				CollationStatus::Seconded if relay_parent_mode.is_enabled() => {
					// Limit is not reached, it's allowed to second another
					// collation.
					fetch_collation(sender, state, pending_collation.clone(), id).await;
				},
				CollationStatus::Seconded => {
					gum::trace!(
						target: LOG_TARGET,
						peer_id = ?peer_id,
						%para_id,
						?relay_parent,
						"A collation has been already seconded",
					);
				},
			}
		},
		Err(AdvertisementError::InvalidArguments) => {
			gum::warn!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				?relay_parent,
				relay_parent_mode = ?relay_parent_mode,
				"Relay parent mode mismatch",
			);
		},
		Err(error) => {
			gum::debug!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				?relay_parent,
				?error,
				"Invalid advertisement",
			);

			modify_reputation(sender, *peer_id, COST_UNEXPECTED_MESSAGE).await;
		},
	}
}

/// Our view has changed.
async fn handle_our_view_change<Sender>(
	sender: &mut Sender,
	state: &mut State,
	keystore: &SyncCryptoStorePtr,
	view: OurView,
) -> Result<()>
where
	Sender: CollatorProtocolSenderTrait,
{
	let current_leaves = state.active_leaves.clone();

	let removed = current_leaves.iter().filter(|(h, _)| !view.contains(*h));
	let added = view.iter().filter(|h| !current_leaves.contains_key(h));

	for leaf in added {
		let mode = prospective_parachains_mode(sender, *leaf).await?;

		if let Some(span) = view.span_per_head().get(leaf).cloned() {
			let per_leaf_span = PerLeafSpan::new(span, "validator-side");
			state.span_per_relay_parent.insert(*leaf, per_leaf_span);
		}

		let mut per_relay_parent = PerRelayParent::new(mode);
		assign_incoming(
			sender,
			&mut per_relay_parent.assignment,
			&mut state.current_assignments,
			keystore,
			*leaf,
		)
		.await;

		state.active_leaves.insert(*leaf, mode);
		state.per_relay_parent.insert(*leaf, per_relay_parent);

		if mode.is_enabled() {
			state
				.implicit_view
				.activate_leaf(sender, *leaf)
				.await
				.map_err(Error::ImplicitViewFetchError)?;

			let allowed_ancestry = state
				.implicit_view
				.known_allowed_relay_parents_under(leaf, None)
				.unwrap_or_default();
			for block_hash in allowed_ancestry {
				if let Entry::Vacant(entry) = state.per_relay_parent.entry(*block_hash) {
					let mut per_relay_parent =
						PerRelayParent::new(ProspectiveParachainsMode::Enabled);
					assign_incoming(
						sender,
						&mut per_relay_parent.assignment,
						&mut state.current_assignments,
						keystore,
						*block_hash,
					)
					.await;

					entry.insert(per_relay_parent);
				}
			}
		}
	}

	for (removed, mode) in removed {
		state.active_leaves.remove(removed);
		// If the leaf is deactivated it still may stay in the view as a part
		// of implicit ancestry. Only update the state after the hash is actually
		// pruned from the block info storage.
		let pruned = if mode.is_enabled() {
			state.implicit_view.deactivate_leaf(*removed)
		} else {
			vec![*removed]
		};

		for removed in pruned {
			if let Some(per_relay_parent) = state.per_relay_parent.remove(&removed) {
				remove_outgoing(&mut state.current_assignments, per_relay_parent);
			}

			state.requested_collations.retain(|k, _| k.relay_parent != removed);
			state.fetched_candidates.retain(|k, _| k.relay_parent != removed);
			state.span_per_relay_parent.remove(&removed);
		}
	}

	for (peer_id, peer_data) in state.peer_data.iter_mut() {
		peer_data.prune_old_advertisements(
			&state.implicit_view,
			&state.active_leaves,
			&state.per_relay_parent,
		);

		// Disconnect peers who are not relevant to our current or next para.
		//
		// If the peer hasn't declared yet, they will be disconnected if they do not
		// declare.
		if let Some(para_id) = peer_data.collating_para() {
			if !state.current_assignments.contains_key(&para_id) {
				gum::trace!(
					target: LOG_TARGET,
					?peer_id,
					?para_id,
					"Disconnecting peer on view change (not current parachain id)"
				);
				disconnect_peer(sender, peer_id.clone()).await;
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
			handle_our_view_change(ctx.sender(), state, keystore, view).await?;
		},
		PeerMessage(remote, msg) => {
			process_incoming_peer_message(ctx, state, remote, msg).await;
		},
		PeerMessage(_, Versioned::VStaging(_)) => todo!(),
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
		DistributeCollation(..) => {
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
			let receipt = match stmt.payload() {
				Statement::Seconded(receipt) => receipt,
				Statement::Valid(_) => {
					// Seconded statement expected.
					return
				},
			};
			let fetched_collation = FetchedCollation::from(&receipt.to_plain());
			if let Some(collation_event) = state.fetched_candidates.remove(&fetched_collation) {
				let (collator_id, pending_collation) = collation_event;
				let PendingCollation { relay_parent, peer_id, .. } = pending_collation;
				note_good_collation(ctx.sender(), &state.peer_data, collator_id.clone()).await;
				notify_collation_seconded(ctx.sender(), peer_id, relay_parent, stmt).await;

				if let Some(state) = state.per_relay_parent.get_mut(&parent) {
					state.collations.status = CollationStatus::Seconded;
					*state
						.collations
						.seconded_count
						.entry(pending_collation.para_id)
						.or_insert(0) += 1;
				}
				// If async backing is enabled, make an attempt to fetch next collation.
				dequeue_next_collation_and_fetch(ctx, state, parent, collator_id).await;
			} else {
				gum::debug!(
					target: LOG_TARGET,
					relay_parent = ?parent,
					"Collation has been seconded, but the relay parent is deactivated",
				);
			}
		},
		Invalid(parent, candidate_receipt) => {
			let fetched_collation = FetchedCollation::from(&candidate_receipt);
			let id = match state.fetched_candidates.entry(fetched_collation) {
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

// wait until next inactivity check. returns the instant for the following check.
async fn wait_until_next_check(last_poll: Instant) -> Instant {
	let now = Instant::now();
	let next_poll = last_poll + ACTIVITY_POLL;

	if next_poll > now {
		Delay::new(next_poll - now).await
	}

	Instant::now()
}

fn infinite_stream(every: Duration) -> impl FusedStream<Item = ()> {
	futures::stream::unfold(Instant::now() + every, |next_check| async move {
		Some(((), wait_until_next_check(next_check).await))
	})
	.fuse()
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

	let next_inactivity_stream = infinite_stream(ACTIVITY_POLL);
	futures::pin_mut!(next_inactivity_stream);

	let check_collations_stream = infinite_stream(CHECK_COLLATIONS_POLL);
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
			reputation_changes.push((pending_collation.peer_id.clone(), rep));
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
	if let Some((next, id)) = state.per_relay_parent.get_mut(&relay_parent).and_then(|state| {
		state
			.collations
			.get_next_collation_to_fetch(Some(&previous_fetch), state.prospective_parachains_mode)
	}) {
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

#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn request_persisted_validation_data<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	para_id: ParaId,
) -> Option<PersistedValidationData> {
	// TODO [https://github.com/paritytech/polkadot/issues/5054]
	//
	// As of https://github.com/paritytech/polkadot/pull/5557 the
	// `Second` message requires the `PersistedValidationData` to be
	// supplied.
	//
	// Without asynchronous backing, this can be easily fetched from the
	// chain state.
	//
	// This assumes the core is _scheduled_, in keeping with the effective
	// current behavior. If the core is occupied, we simply don't return
	// anything. Likewise with runtime API errors, which are rare.
	let res = polkadot_node_subsystem_util::request_persisted_validation_data(
		relay_parent,
		para_id,
		OccupiedCoreAssumption::Free,
		ctx.sender(),
	)
	.await
	.await;

	match res {
		Ok(Ok(Some(pvd))) => Some(pvd),
		_ => None,
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

	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(state) => state,
		None => {
			gum::trace!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				"Fetched collation for a parent out of view",
			);
			return
		},
	};

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

	let collations = &mut per_relay_parent.collations;
	// There's always a single collation being fetched at any moment of time.
	// In case of a failure, we reset the status back to waiting.
	collations.status = CollationStatus::WaitingOnValidation;

	let fetched_collation = FetchedCollation::from(&candidate_receipt);
	if let Entry::Vacant(entry) = state.fetched_candidates.entry(fetched_collation) {
		collation_event.1.commitments_hash = Some(candidate_receipt.commitments_hash);

		if let Some(pvd) = request_persisted_validation_data(
			ctx,
			candidate_receipt.descriptor().relay_parent,
			candidate_receipt.descriptor().para_id,
		)
		.await
		{
			// TODO [https://github.com/paritytech/polkadot/issues/5054]
			//
			// If PVD isn't available (core occupied) then we'll silently
			// just not second this. But prior to asynchronous backing
			// we wouldn't second anyway because the core is occupied.
			//
			// The proper refactoring would be to accept declares from collators
			// but not even fetch from them if the core is occupied. Given 5054,
			// there's no reason to do this right now.
			ctx.send_message(CandidateBackingMessage::Second(
				relay_parent.clone(),
				candidate_receipt,
				pvd,
				pov,
			))
			.await;
		}

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
			disconnect_peer(sender, peer.clone()).await;
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
			Ok(request_v1::CollationFetchingResponse::Collation(receipt, _))
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
			Ok(request_v1::CollationFetchingResponse::Collation(receipt, pov)) => {
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
