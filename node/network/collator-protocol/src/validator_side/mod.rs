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
	convert::TryInto,
	iter::FromIterator,
	task::Poll,
	time::{Duration, Instant},
};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::{CollationVersion, PeerSet},
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
		CanSecondRequest, CandidateBackingMessage, CollatorProtocolMessage, IfDisconnected,
		NetworkBridgeEvent, NetworkBridgeTxMessage, ProspectiveParachainsMessage,
		ProspectiveValidationDataRequest,
	},
	overseer, CollatorProtocolSenderTrait, FromOrchestra, OverseerSignal, PerLeafSpan,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::View as ImplicitView,
	metrics::prometheus::prometheus::HistogramTimer,
	runtime::{prospective_parachains_mode, ProspectiveParachainsMode},
};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, CollatorId, CoreState, Hash, Id as ParaId,
	OccupiedCoreAssumption, PersistedValidationData,
};

use crate::error::{Error, FetchError, Result, SecondingError};

use super::{modify_reputation, tick_stream, LOG_TARGET};

mod collation;
mod metrics;

use collation::{
	fetched_collation_sanity_check, BlockedAdvertisement, CollationEvent, CollationStatus,
	Collations, FetchedCollation, PendingCollation, PendingCollationFetch, ProspectiveCandidate,
};

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
const CHECK_COLLATIONS_POLL: Duration = Duration::from_millis(5);

struct PerRequest {
	/// Responses from collator.
	///
	/// The response payload is the same for both versions of protocol
	/// and doesn't have vstaging alias for simplicity.
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
	/// Collations advertised by peer per relay parent.
	///
	/// V1 network protocol doesn't include candidate hash in
	/// advertisements, we store an empty set in this case to occupy
	/// a slot in map.
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
enum InsertAdvertisementError {
	/// Advertisement is already known.
	Duplicate,
	/// Collation relay parent is out of our view.
	OutOfOurView,
	/// No prior declare message received.
	UndeclaredCollator,
	/// A limit for announcements per peer is reached.
	PeerLimitReached,
	/// Mismatch of relay parent mode and advertisement arguments.
	/// An internal error that should not happen.
	ProtocolMismatch,
}

#[derive(Debug)]
struct PeerData {
	view: View,
	state: PeerState,
	version: CollationVersion,
}

impl PeerData {
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
				// Remove relay parent advertisements if it went out
				// of our (implicit) view.
				let keep = per_relay_parent
					.get(removed)
					.map(|s| {
						is_relay_parent_in_implicit_view(
							removed,
							s.prospective_parachains_mode,
							implicit_view,
							active_leaves,
							peer_state.para_id,
						)
					})
					.unwrap_or(false);

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
					is_relay_parent_in_implicit_view(
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
	) -> std::result::Result<(CollatorId, ParaId), InsertAdvertisementError> {
		match self.state {
			PeerState::Connected(_) => Err(InsertAdvertisementError::UndeclaredCollator),
			PeerState::Collating(ref mut state) => {
				if !is_relay_parent_in_implicit_view(
					&on_relay_parent,
					relay_parent_mode,
					implicit_view,
					active_leaves,
					state.para_id,
				) {
					return Err(InsertAdvertisementError::OutOfOurView)
				}

				match (relay_parent_mode, candidate_hash) {
					(ProspectiveParachainsMode::Disabled, candidate_hash) => {
						if state.advertisements.contains_key(&on_relay_parent) {
							return Err(InsertAdvertisementError::Duplicate)
						}
						state
							.advertisements
							.insert(on_relay_parent, HashSet::from_iter(candidate_hash));
					},
					(
						ProspectiveParachainsMode::Enabled { max_candidate_depth, .. },
						Some(candidate_hash),
					) => {
						if state
							.advertisements
							.get(&on_relay_parent)
							.map_or(false, |candidates| candidates.contains(&candidate_hash))
						{
							return Err(InsertAdvertisementError::Duplicate)
						}
						let candidates = state.advertisements.entry(on_relay_parent).or_default();

						if candidates.len() >= max_candidate_depth + 1 {
							return Err(InsertAdvertisementError::PeerLimitReached)
						}
						candidates.insert(candidate_hash);
					},
					_ => return Err(InsertAdvertisementError::ProtocolMismatch),
				}

				state.last_active = Instant::now();
				Ok((state.collator_id.clone(), state.para_id))
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

	/// Note that a peer is now collating with the given collator and para id.
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

#[derive(Debug)]
struct GroupAssignments {
	/// Current assignment.
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

	/// State tracked per relay parent.
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

	/// Advertisements that were accepted as valid by collator protocol but rejected by backing.
	///
	/// It's only legal to fetch collations that are either built on top of the root
	/// of some fragment tree or have a parent node which represents backed candidate.
	/// Otherwise, a validator will keep such advertisement in the memory and re-trigger
	/// requests to backing on new backed candidates and activations.
	blocked_advertisements: HashMap<(ParaId, Hash), Vec<BlockedAdvertisement>>,

	/// Keep track of all fetch collation requests
	collation_fetches: FuturesUnordered<BoxFuture<'static, PendingCollationFetch>>,

	/// When a timer in this `FuturesUnordered` triggers, we should dequeue the next request
	/// attempt in the corresponding `collations_per_relay_parent`.
	///
	/// A triggering timer means that the fetching took too long for our taste and we should give
	/// another collator the chance to be faster (dequeue next fetch request as well).
	collation_fetch_timeouts:
		FuturesUnordered<BoxFuture<'static, (CollatorId, Option<CandidateHash>, Hash)>>,

	/// Collations that we have successfully requested from peers and waiting
	/// on validation.
	fetched_candidates: HashMap<FetchedCollation, CollationEvent>,
}

fn is_relay_parent_in_implicit_view(
	relay_parent: &Hash,
	relay_parent_mode: ProspectiveParachainsMode,
	implicit_view: &ImplicitView,
	active_leaves: &HashMap<Hash, ProspectiveParachainsMode>,
	para_id: ParaId,
) -> bool {
	match relay_parent_mode {
		ProspectiveParachainsMode::Disabled => active_leaves.contains_key(relay_parent),
		ProspectiveParachainsMode::Enabled { .. } => active_leaves.iter().any(|(hash, mode)| {
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
	relay_parent_mode: ProspectiveParachainsMode,
) -> Result<()>
where
	Sender: CollatorProtocolSenderTrait,
{
	let validators = polkadot_node_subsystem_util::request_validators(relay_parent, sender)
		.await
		.await
		.map_err(Error::CancelledActiveValidators)??;

	let (groups, rotation_info) =
		polkadot_node_subsystem_util::request_validator_groups(relay_parent, sender)
			.await
			.await
			.map_err(Error::CancelledValidatorGroups)??;

	let cores = polkadot_node_subsystem_util::request_availability_cores(relay_parent, sender)
		.await
		.await
		.map_err(Error::CancelledAvailabilityCores)??;

	let para_now = match polkadot_node_subsystem_util::signing_key_and_index(&validators, keystore)
		.await
		.and_then(|(_, index)| polkadot_node_subsystem_util::find_validator_group(&groups, index))
	{
		Some(group) => {
			let core_now = rotation_info.core_for_group(group, cores.len());

			cores.get(core_now.0 as usize).and_then(|c| match c {
				CoreState::Occupied(core) if relay_parent_mode.is_enabled() => Some(core.para_id()),
				CoreState::Scheduled(core) => Some(core.para_id),
				CoreState::Occupied(_) | CoreState::Free => None,
			})
		},
		None => {
			gum::trace!(target: LOG_TARGET, ?relay_parent, "Not a validator");

			return Ok(())
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
	if let Some(para_id) = para_now.as_ref() {
		let entry = current_assignments.entry(*para_id).or_default();
		*entry += 1;
		if *entry == 1 {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				para_id = ?para_id,
				"Assigned to a parachain",
			);
		}
	}

	*group_assignment = GroupAssignments { current: para_now };

	Ok(())
}

fn remove_outgoing(
	current_assignments: &mut HashMap<ParaId, usize>,
	per_relay_parent: PerRelayParent,
) {
	let GroupAssignments { current, .. } = per_relay_parent.assignment;

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
) -> std::result::Result<(), FetchError> {
	let (tx, rx) = oneshot::channel();

	let PendingCollation { relay_parent, peer_id, prospective_candidate, .. } = pc;
	let candidate_hash = prospective_candidate.as_ref().map(ProspectiveCandidate::candidate_hash);

	let peer_data = state.peer_data.get(&peer_id).ok_or(FetchError::UnknownPeer)?;

	if peer_data.has_advertised(&relay_parent, candidate_hash) {
		request_collation(sender, state, pc, id.clone(), peer_data.version, tx).await?;
		let timeout = |collator_id, candidate_hash, relay_parent| async move {
			Delay::new(MAX_UNSHARED_DOWNLOAD_TIME).await;
			(collator_id, candidate_hash, relay_parent)
		};
		state
			.collation_fetch_timeouts
			.push(timeout(id.clone(), candidate_hash, relay_parent).boxed());
		state.collation_fetches.push(rx.map(move |r| ((id, pc), r)).boxed());

		Ok(())
	} else {
		Err(FetchError::NotAdvertised)
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
	version: CollationVersion,
	relay_parent: Hash,
	statement: SignedFullStatement,
) {
	let statement = statement.into();
	let wire_message = match version {
		CollationVersion::V1 => Versioned::V1(protocol_v1::CollationProtocol::CollatorProtocol(
			protocol_v1::CollatorProtocolMessage::CollationSeconded(relay_parent, statement),
		)),
		CollationVersion::VStaging =>
			Versioned::VStaging(protocol_vstaging::CollationProtocol::CollatorProtocol(
				protocol_vstaging::CollatorProtocolMessage::CollationSeconded(
					relay_parent,
					statement,
				),
			)),
	};
	sender
		.send_message(NetworkBridgeTxMessage::SendCollationMessage(vec![peer_id], wire_message))
		.await;
}

/// A peer's view has changed. A number of things should be done:
///  - Ongoing collation requests have to be canceled.
///  - Advertisements by this peer that are no longer relevant have to be removed.
fn handle_peer_view_change(state: &mut State, peer_id: PeerId, view: View) {
	let peer_data = match state.peer_data.get_mut(&peer_id) {
		Some(peer_data) => peer_data,
		None => return,
	};

	peer_data.update_view(
		&state.implicit_view,
		&state.active_leaves,
		&state.per_relay_parent,
		view,
	);
	state
		.requested_collations
		.retain(|pc, _| pc.peer_id != peer_id || peer_data.has_advertised(&pc.relay_parent, None));
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
	pending_collation: PendingCollation,
	collator_id: CollatorId,
	peer_protocol_version: CollationVersion,
	result: oneshot::Sender<(CandidateReceipt, PoV)>,
) -> std::result::Result<(), FetchError> {
	if state.requested_collations.contains_key(&pending_collation) {
		return Err(FetchError::AlreadyRequested)
	}

	let PendingCollation { relay_parent, para_id, peer_id, prospective_candidate, .. } =
		pending_collation;
	let per_relay_parent = state
		.per_relay_parent
		.get_mut(&relay_parent)
		.ok_or(FetchError::RelayParentOutOfView)?;

	// Relay parent mode is checked in `handle_advertisement`.
	let (requests, response_recv) = match (peer_protocol_version, prospective_candidate) {
		(CollationVersion::V1, _) => {
			let (req, response_recv) = OutgoingRequest::new(
				Recipient::Peer(peer_id),
				request_v1::CollationFetchingRequest { relay_parent, para_id },
			);
			let requests = Requests::CollationFetchingV1(req);
			(requests, response_recv.boxed())
		},
		(CollationVersion::VStaging, Some(ProspectiveCandidate { candidate_hash, .. })) => {
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
		_ => return Err(FetchError::ProtocolMismatch),
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

	state.requested_collations.insert(pending_collation, per_request);

	gum::debug!(
		target: LOG_TARGET,
		peer_id = %peer_id,
		%para_id,
		?relay_parent,
		"Requesting collation",
	);

	let maybe_candidate_hash =
		prospective_candidate.as_ref().map(ProspectiveCandidate::candidate_hash);
	per_relay_parent.collations.status = CollationStatus::Fetching;
	per_relay_parent
		.collations
		.fetching_from
		.replace((collator_id, maybe_candidate_hash));

	sender
		.send_message(NetworkBridgeTxMessage::SendRequests(
			vec![requests],
			IfDisconnected::ImmediateError,
		))
		.await;
	Ok(())
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
					"Peer is already in the collating state",
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

				modify_reputation(ctx.sender(), origin, COST_UNNEEDED_COLLATOR).await;
				gum::trace!(target: LOG_TARGET, "Disconnecting unneeded collator");
				disconnect_peer(ctx.sender(), origin).await;
			}
		},
		Versioned::V1(V1::AdvertiseCollation(relay_parent)) =>
			if let Err(err) =
				handle_advertisement(ctx.sender(), state, relay_parent, origin, None).await
			{
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?relay_parent,
					error = ?err,
					"Rejected v1 advertisement",
				);

				if let Some(rep) = err.reputation_changes() {
					modify_reputation(ctx.sender(), origin.clone(), rep).await;
				}
			},
		Versioned::VStaging(VStaging::AdvertiseCollation {
			relay_parent,
			candidate_hash,
			parent_head_data_hash,
		}) =>
			if let Err(err) = handle_advertisement(
				ctx.sender(),
				state,
				relay_parent,
				origin,
				Some((candidate_hash, parent_head_data_hash)),
			)
			.await
			{
				gum::debug!(
					target: LOG_TARGET,
					peer_id = ?origin,
					?relay_parent,
					?candidate_hash,
					error = ?err,
					"Rejected vstaging advertisement",
				);

				if let Some(rep) = err.reputation_changes() {
					modify_reputation(ctx.sender(), origin.clone(), rep).await;
				}
			},
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

#[derive(Debug)]
enum AdvertisementError {
	/// Relay parent is unknown.
	RelayParentUnknown,
	/// Peer is not present in the subsystem state.
	UnknownPeer,
	/// Peer has not declared its para id.
	UndeclaredCollator,
	/// We're assigned to a different para at the given relay parent.
	InvalidAssignment,
	/// An advertisement format doesn't match the relay parent.
	ProtocolMismatch,
	/// Para reached a limit of seconded candidates for this relay parent.
	SecondedLimitReached,
	/// Advertisement is invalid.
	Invalid(InsertAdvertisementError),
}

impl AdvertisementError {
	fn reputation_changes(&self) -> Option<Rep> {
		use AdvertisementError::*;
		match self {
			InvalidAssignment => Some(COST_WRONG_PARA),
			RelayParentUnknown | UndeclaredCollator | Invalid(_) => Some(COST_UNEXPECTED_MESSAGE),
			UnknownPeer | ProtocolMismatch | SecondedLimitReached => None,
		}
	}
}

// Requests backing to sanity check the advertisement.
async fn can_second<Sender>(
	sender: &mut Sender,
	candidate_para_id: ParaId,
	candidate_relay_parent: Hash,
	candidate_hash: CandidateHash,
	parent_head_data_hash: Hash,
) -> bool
where
	Sender: CollatorProtocolSenderTrait,
{
	let request = CanSecondRequest {
		candidate_para_id,
		candidate_relay_parent,
		candidate_hash,
		parent_head_data_hash,
	};
	let (tx, rx) = oneshot::channel();
	sender.send_message(CandidateBackingMessage::CanSecond(request, tx)).await;

	rx.await.unwrap_or_else(|err| {
		gum::warn!(
			target: LOG_TARGET,
			?err,
			?candidate_relay_parent,
			?candidate_para_id,
			?candidate_hash,
			"CanSecond-request responder was dropped",
		);

		false
	})
}

/// Checks whether any of the advertisements are unblocked and attempts to fetch them.
async fn request_unblocked_collations<Sender, I>(sender: &mut Sender, state: &mut State, blocked: I)
where
	Sender: CollatorProtocolSenderTrait,
	I: IntoIterator<Item = ((ParaId, Hash), Vec<BlockedAdvertisement>)>,
{
	for (key, mut value) in blocked {
		let (para_id, para_head) = key;
		let blocked = std::mem::take(&mut value);
		for blocked in blocked {
			let is_seconding_allowed = can_second(
				sender,
				para_id,
				blocked.candidate_relay_parent,
				blocked.candidate_hash,
				para_head,
			)
			.await;

			if is_seconding_allowed {
				let result = enqueue_collation(
					sender,
					state,
					blocked.candidate_relay_parent,
					para_id,
					blocked.peer_id,
					blocked.collator_id,
					Some((blocked.candidate_hash, para_head)),
				)
				.await;
				if let Err(fetch_error) = result {
					gum::debug!(
						target: LOG_TARGET,
						relay_parent = ?blocked.candidate_relay_parent,
						para_id = ?para_id,
						peer_id = ?blocked.peer_id,
						error = %fetch_error,
						"Failed to request unblocked collation",
					);
				}
			} else {
				// Keep the advertisement.
				value.push(blocked);
			}
		}

		if !value.is_empty() {
			state.blocked_advertisements.insert(key, value);
		}
	}
}

async fn handle_advertisement<Sender>(
	sender: &mut Sender,
	state: &mut State,
	relay_parent: Hash,
	peer_id: PeerId,
	prospective_candidate: Option<(CandidateHash, Hash)>,
) -> std::result::Result<(), AdvertisementError>
where
	Sender: CollatorProtocolSenderTrait,
{
	let _span = state
		.span_per_relay_parent
		.get(&relay_parent)
		.map(|s| s.child("advertise-collation"));

	let per_relay_parent = state
		.per_relay_parent
		.get(&relay_parent)
		.ok_or(AdvertisementError::RelayParentUnknown)?;

	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;
	let assignment = &per_relay_parent.assignment;

	let peer_data = state.peer_data.get_mut(&peer_id).ok_or(AdvertisementError::UnknownPeer)?;
	let collator_para_id =
		peer_data.collating_para().ok_or(AdvertisementError::UndeclaredCollator)?;

	match assignment.current {
		Some(id) if id == collator_para_id => {
			// Our assignment.
		},
		_ => return Err(AdvertisementError::InvalidAssignment),
	};

	if relay_parent_mode.is_enabled() && prospective_candidate.is_none() {
		// Expected vstaging advertisement.
		return Err(AdvertisementError::ProtocolMismatch)
	}

	// Always insert advertisements that pass all the checks for spam protection.
	let candidate_hash = prospective_candidate.map(|(hash, ..)| hash);
	let (collator_id, para_id) = peer_data
		.insert_advertisement(
			relay_parent,
			relay_parent_mode,
			candidate_hash,
			&state.implicit_view,
			&state.active_leaves,
		)
		.map_err(AdvertisementError::Invalid)?;
	if !per_relay_parent.collations.is_seconded_limit_reached(relay_parent_mode) {
		return Err(AdvertisementError::SecondedLimitReached)
	}

	if let Some((candidate_hash, parent_head_data_hash)) = prospective_candidate {
		let is_seconding_allowed = !relay_parent_mode.is_enabled() ||
			can_second(
				sender,
				collator_para_id,
				relay_parent,
				candidate_hash,
				parent_head_data_hash,
			)
			.await;

		if !is_seconding_allowed {
			gum::debug!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				para_id = ?para_id,
				?candidate_hash,
				"Seconding is not allowed by backing, queueing advertisement",
			);
			state
				.blocked_advertisements
				.entry((collator_para_id, parent_head_data_hash))
				.or_default()
				.push(BlockedAdvertisement {
					peer_id,
					collator_id: collator_id.clone(),
					candidate_relay_parent: relay_parent,
					candidate_hash,
				});

			return Ok(())
		}
	}

	let result = enqueue_collation(
		sender,
		state,
		relay_parent,
		para_id,
		peer_id,
		collator_id,
		prospective_candidate,
	)
	.await;
	if let Err(fetch_error) = result {
		gum::debug!(
			target: LOG_TARGET,
			relay_parent = ?relay_parent,
			para_id = ?para_id,
			peer_id = ?peer_id,
			error = %fetch_error,
			"Failed to request advertised collation",
		);
	}

	Ok(())
}

/// Enqueue collation for fetching. The advertisement is expected to be
/// validated.
async fn enqueue_collation<Sender>(
	sender: &mut Sender,
	state: &mut State,
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	collator_id: CollatorId,
	prospective_candidate: Option<(CandidateHash, Hash)>,
) -> std::result::Result<(), FetchError>
where
	Sender: CollatorProtocolSenderTrait,
{
	gum::debug!(
		target: LOG_TARGET,
		peer_id = ?peer_id,
		%para_id,
		?relay_parent,
		"Received advertise collation",
	);
	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(rp_state) => rp_state,
		None => {
			// Race happened, not an error.
			gum::trace!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				%para_id,
				?relay_parent,
				?prospective_candidate,
				"Candidate relay parent went out of view for valid advertisement",
			);
			return Ok(())
		},
	};
	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;
	let prospective_candidate =
		prospective_candidate.map(|(candidate_hash, parent_head_data_hash)| ProspectiveCandidate {
			candidate_hash,
			parent_head_data_hash,
		});

	let collations = &mut per_relay_parent.collations;
	if !collations.is_seconded_limit_reached(relay_parent_mode) {
		gum::trace!(
			target: LOG_TARGET,
			peer_id = ?peer_id,
			%para_id,
			?relay_parent,
			"Limit of seconded collations reached for valid advertisement",
		);
		return Ok(())
	}

	let pending_collation =
		PendingCollation::new(relay_parent, para_id, &peer_id, prospective_candidate);

	match collations.status {
		CollationStatus::Fetching | CollationStatus::WaitingOnValidation => {
			gum::trace!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				%para_id,
				?relay_parent,
				"Added collation to the pending list"
			);
			collations.waiting_queue.push_back((pending_collation, collator_id));
		},
		CollationStatus::Waiting => {
			fetch_collation(sender, state, pending_collation, collator_id).await?;
		},
		CollationStatus::Seconded if relay_parent_mode.is_enabled() => {
			// Limit is not reached, it's allowed to second another
			// collation.
			fetch_collation(sender, state, pending_collation, collator_id).await?;
		},
		CollationStatus::Seconded => {
			gum::trace!(
				target: LOG_TARGET,
				peer_id = ?peer_id,
				%para_id,
				?relay_parent,
				?relay_parent_mode,
				"A collation has already been seconded",
			);
		},
	}

	Ok(())
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
			mode,
		)
		.await?;

		state.active_leaves.insert(*leaf, mode);
		state.per_relay_parent.insert(*leaf, per_relay_parent);

		if mode.is_enabled() {
			state
				.implicit_view
				.activate_leaf(sender, *leaf)
				.await
				.map_err(Error::ImplicitViewFetchError)?;

			// Order is always descending.
			let allowed_ancestry = state
				.implicit_view
				.known_allowed_relay_parents_under(leaf, None)
				.unwrap_or_default();
			for block_hash in allowed_ancestry {
				if let Entry::Vacant(entry) = state.per_relay_parent.entry(*block_hash) {
					let mut per_relay_parent = PerRelayParent::new(mode);
					assign_incoming(
						sender,
						&mut per_relay_parent.assignment,
						&mut state.current_assignments,
						keystore,
						*block_hash,
						mode,
					)
					.await?;

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
	// Remove blocked advertisements that left the view.
	state.blocked_advertisements.retain(|_, ads| {
		ads.retain(|ad| state.per_relay_parent.contains_key(&ad.candidate_relay_parent));

		!ads.is_empty()
	});
	// Re-trigger previously failed requests again.
	//
	// This makes sense for several reasons, one simple example: if a hypothetical depth
	// for an advertisement initially exceeded the limit and the candidate was included
	// in a new leaf.
	let maybe_unblocked = std::mem::take(&mut state.blocked_advertisements);
	// Could be optimized to only sanity check new leaves.
	request_unblocked_collations(sender, state, maybe_unblocked).await;

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
		PeerConnected(peer_id, observed_role, protocol_version, _) => {
			let version = match protocol_version.try_into() {
				Ok(version) => version,
				Err(err) => {
					// Network bridge is expected to handle this.
					gum::error!(
						target: LOG_TARGET,
						?peer_id,
						?observed_role,
						?err,
						"Unsupported protocol version"
					);
					return Ok(())
				},
			};
			state.peer_data.entry(peer_id).or_insert_with(|| PeerData {
				view: View::default(),
				state: PeerState::Connected(Instant::now()),
				version,
			});
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
			handle_peer_view_change(state, peer_id, view);
		},
		OurViewChange(view) => {
			handle_our_view_change(ctx.sender(), state, keystore, view).await?;
		},
		PeerMessage(remote, msg) => {
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
					gum::warn!(
						target: LOG_TARGET,
						?stmt,
						relay_parent = %parent,
						"Seconded message received with a `Valid` statement",
					);
					return
				},
			};
			let fetched_collation = FetchedCollation::from(&receipt.to_plain());
			if let Some(collation_event) = state.fetched_candidates.remove(&fetched_collation) {
				let (collator_id, pending_collation) = collation_event;
				let PendingCollation { relay_parent, peer_id, prospective_candidate, .. } =
					pending_collation;
				note_good_collation(ctx.sender(), &state.peer_data, collator_id.clone()).await;
				if let Some(peer_data) = state.peer_data.get(&peer_id) {
					notify_collation_seconded(
						ctx.sender(),
						peer_id,
						peer_data.version,
						relay_parent,
						stmt,
					)
					.await;
				}

				if let Some(rp_state) = state.per_relay_parent.get_mut(&parent) {
					rp_state.collations.status = CollationStatus::Seconded;
					rp_state.collations.note_seconded();
				}
				// If async backing is enabled, make an attempt to fetch next collation.
				let maybe_candidate_hash =
					prospective_candidate.as_ref().map(ProspectiveCandidate::candidate_hash);
				dequeue_next_collation_and_fetch(
					ctx,
					state,
					parent,
					(collator_id, maybe_candidate_hash),
				)
				.await;
			} else {
				gum::debug!(
					target: LOG_TARGET,
					relay_parent = ?parent,
					"Collation has been seconded, but the relay parent is deactivated",
				);
			}
		},
		Backed { para_id, para_head } => {
			let maybe_unblocked = state.blocked_advertisements.remove_entry(&(para_id, para_head));
			request_unblocked_collations(ctx.sender(), state, maybe_unblocked).await;
		},
		Invalid(parent, candidate_receipt) => {
			let fetched_collation = FetchedCollation::from(&candidate_receipt);
			let candidate_hash = fetched_collation.candidate_hash;
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

			dequeue_next_collation_and_fetch(ctx, state, parent, (id, Some(candidate_hash))).await;
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
				let (collator_id, pc) = res.0.clone();
				if let Err(err) = kick_off_seconding(&mut ctx, &mut state, res).await {
					gum::warn!(
						target: LOG_TARGET,
						relay_parent = ?pc.relay_parent,
						para_id = ?pc.para_id,
						peer_id = ?pc.peer_id,
						error = %err,
						"Seconding aborted due to an error",
					);

					if err.is_malicious() {
						// Report malicious peer.
						modify_reputation(ctx.sender(), pc.peer_id, COST_REPORT_BAD).await;
					}
					let maybe_candidate_hash =
						pc.prospective_candidate.as_ref().map(ProspectiveCandidate::candidate_hash);
					dequeue_next_collation_and_fetch(
						&mut ctx,
						&mut state,
						pc.relay_parent,
						(collator_id, maybe_candidate_hash),
					)
					.await;
				}
			}
			res = state.collation_fetch_timeouts.select_next_some() => {
				let (collator_id, maybe_candidate_hash, relay_parent) = res;
				gum::debug!(
					target: LOG_TARGET,
					?relay_parent,
					?collator_id,
					"Timeout hit - already seconded?"
				);
				dequeue_next_collation_and_fetch(
					&mut ctx,
					&mut state,
					relay_parent,
					(collator_id, maybe_candidate_hash),
				)
				.await;
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
	// The collator we tried to fetch from last, optionally which candidate.
	previous_fetch: (CollatorId, Option<CandidateHash>),
) {
	while let Some((next, id)) = state.per_relay_parent.get_mut(&relay_parent).and_then(|state| {
		state
			.collations
			.get_next_collation_to_fetch(&previous_fetch, state.prospective_parachains_mode)
	}) {
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			?id,
			"Successfully dequeued next advertisement - fetching ..."
		);
		if let Err(err) = fetch_collation(ctx.sender(), state, next, id).await {
			gum::debug!(
				target: LOG_TARGET,
				relay_parent = ?next.relay_parent,
				para_id = ?next.para_id,
				peer_id = ?next.peer_id,
				error = %err,
				"Failed to request a collation, dequeueing next one",
			);
		} else {
			break
		}
	}
}

async fn request_persisted_validation_data<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	para_id: ParaId,
) -> std::result::Result<Option<PersistedValidationData>, SecondingError>
where
	Sender: CollatorProtocolSenderTrait,
{
	// The core is guaranteed to be scheduled since we accepted the advertisement.
	polkadot_node_subsystem_util::request_persisted_validation_data(
		relay_parent,
		para_id,
		OccupiedCoreAssumption::Free,
		sender,
	)
	.await
	.await
	.map_err(SecondingError::CancelledRuntimePersistedValidationData)?
	.map_err(SecondingError::RuntimeApi)
}

async fn request_prospective_validation_data<Sender>(
	sender: &mut Sender,
	candidate_relay_parent: Hash,
	parent_head_data_hash: Hash,
	para_id: ParaId,
) -> std::result::Result<Option<PersistedValidationData>, SecondingError>
where
	Sender: CollatorProtocolSenderTrait,
{
	let (tx, rx) = oneshot::channel();

	let request =
		ProspectiveValidationDataRequest { para_id, candidate_relay_parent, parent_head_data_hash };

	sender
		.send_message(ProspectiveParachainsMessage::GetProspectiveValidationData(request, tx))
		.await;

	rx.await.map_err(SecondingError::CancelledProspectiveValidationData)
}

/// Handle a fetched collation result.
#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
async fn kick_off_seconding<Context>(
	ctx: &mut Context,
	state: &mut State,
	(mut collation_event, res): PendingCollationFetch,
) -> std::result::Result<(), SecondingError> {
	let relay_parent = collation_event.1.relay_parent;
	let para_id = collation_event.1.para_id;

	let per_relay_parent = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(state) => state,
		None => {
			// Relay parent went out of view, not an error.
			gum::trace!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				"Fetched collation for a parent out of view",
			);
			return Ok(())
		},
	};
	let collations = &mut per_relay_parent.collations;
	let relay_parent_mode = per_relay_parent.prospective_parachains_mode;

	let (candidate_receipt, pov) = res?;

	let fetched_collation = FetchedCollation::from(&candidate_receipt);
	if let Entry::Vacant(entry) = state.fetched_candidates.entry(fetched_collation) {
		collation_event.1.commitments_hash = Some(candidate_receipt.commitments_hash);

		let pvd = match (relay_parent_mode, collation_event.1.prospective_candidate) {
			(
				ProspectiveParachainsMode::Enabled { .. },
				Some(ProspectiveCandidate { parent_head_data_hash, .. }),
			) =>
				request_prospective_validation_data(
					ctx.sender(),
					relay_parent,
					parent_head_data_hash,
					para_id,
				)
				.await?,
			(ProspectiveParachainsMode::Disabled, _) =>
				request_persisted_validation_data(
					ctx.sender(),
					candidate_receipt.descriptor().relay_parent,
					candidate_receipt.descriptor().para_id,
				)
				.await?,
			_ => {
				// `handle_advertisement` checks for protocol mismatch.
				return Ok(())
			},
		}
		.ok_or(SecondingError::PersistedValidationDataNotFound)?;

		fetched_collation_sanity_check(&collation_event.1, &candidate_receipt, &pvd)?;

		ctx.send_message(CandidateBackingMessage::Second(
			relay_parent,
			candidate_receipt,
			pvd,
			pov,
		))
		.await;
		// There's always a single collation being fetched at any moment of time.
		// In case of a failure, we reset the status back to waiting.
		collations.status = CollationStatus::WaitingOnValidation;

		entry.insert(collation_event);
		Ok(())
	} else {
		Err(SecondingError::Duplicate)
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
