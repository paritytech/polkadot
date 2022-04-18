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

//! [`ApprovalDistributionSubsystem`] implementation.
//!
//! https://w3f.github.io/parachain-implementers-guide/node/approval/approval-distribution.html

#![warn(missing_docs)]

use dashmap::DashMap;
use futures::{channel::oneshot, stream::StreamExt, FutureExt as _, SinkExt};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, PeerId, UnifiedReputationChange as Rep, View,
};
use polkadot_node_primitives::approval::{
	AssignmentCert, BlockApprovalMeta, IndirectAssignmentCert, IndirectSignedApprovalVote,
};
use polkadot_node_subsystem::{
	messages::{
		network_bridge_event, ApprovalCheckResult, ApprovalDistributionMessage,
		ApprovalVotingMessage, AssignmentCheckResult, NetworkBridgeEvent, NetworkBridgeMessage,
	},
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext,
	SubsystemError, SubsystemSender,
};
use polkadot_primitives::v2::{
	BlockNumber, CandidateIndex, Hash, SessionIndex, ValidatorIndex, ValidatorSignature,
};
use rand::{CryptoRng, Rng, SeedableRng};
use std::collections::{hash_map, BTreeMap, HashMap, HashSet, VecDeque};

use self::metrics::Metrics;
use polkadot_node_subsystem::messages::AllMessages::{ApprovalVoting, NetworkBridge};
use std::sync::{Arc, Mutex};
mod metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::approval-distribution";
const WORKER_COUNT: u32 = 8;
const COST_UNEXPECTED_MESSAGE: Rep =
	Rep::CostMinor("Peer sent an out-of-view assignment or approval");
const COST_DUPLICATE_MESSAGE: Rep = Rep::CostMinorRepeated("Peer sent identical messages");
const COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE: Rep =
	Rep::CostMinor("The vote was valid but too far in the future");
const COST_INVALID_MESSAGE: Rep = Rep::CostMajor("The vote was bad");

const BENEFIT_VALID_MESSAGE: Rep = Rep::BenefitMinor("Peer sent a valid message");
const BENEFIT_VALID_MESSAGE_FIRST: Rep =
	Rep::BenefitMinorFirst("Valid message with new information");

/// The number of peers to randomly propagate messages to.
const RANDOM_CIRCULATION: usize = 4;
/// The sample rate for randomly propagating messages. This
/// reduces the left tail of the binomial distribution but also
/// introduces a bias towards peers who we sample before others
/// (i.e. those who get a block before others).
const RANDOM_SAMPLE_RATE: usize = polkadot_node_subsystem_util::MIN_GOSSIP_PEERS;

/// The Approval Distribution subsystem.
pub struct ApprovalDistribution {
	metrics: Metrics,
}

/// The approval distribution worker
pub struct ApprovalDistributionWorker {
	metrics: Metrics,
	proxy_receiver: futures::channel::mpsc::Receiver<FromOverseer<ApprovalDistributionMessage>>,
}

/// Contains recently finalized
/// or those pruned due to finalization.
#[derive(Default)]
struct RecentlyOutdated {
	buf: VecDeque<Hash>,
}

impl RecentlyOutdated {
	fn note_outdated(&mut self, hash: Hash) {
		const MAX_BUF_LEN: usize = 20;

		self.buf.push_back(hash);

		while self.buf.len() > MAX_BUF_LEN {
			let _ = self.buf.pop_front();
		}
	}

	fn is_recent_outdated(&self, hash: &Hash) -> bool {
		self.buf.contains(hash)
	}
}

#[derive(Clone)]
struct SessionTopology {
	peers_x: HashSet<PeerId>,
	validator_indices_x: HashSet<ValidatorIndex>,
	peers_y: HashSet<PeerId>,
	validator_indices_y: HashSet<ValidatorIndex>,
}

impl SessionTopology {
	// Given the originator of a message, indicates the part of the topology
	// we're meant to send the message to.
	fn required_routing_for(&self, originator: ValidatorIndex, local: bool) -> RequiredRouting {
		if local {
			return RequiredRouting::GridXY
		}

		let grid_x = self.validator_indices_x.contains(&originator);
		let grid_y = self.validator_indices_y.contains(&originator);

		match (grid_x, grid_y) {
			(false, false) => RequiredRouting::None,
			(true, false) => RequiredRouting::GridY, // messages from X go to Y
			(false, true) => RequiredRouting::GridX, // messages from Y go to X
			(true, true) => RequiredRouting::GridXY, // if the grid works as expected, this shouldn't happen.
		}
	}

	// Get a filter function based on this topology and the required routing
	// which returns `true` for peers that are within the required routing set
	// and false otherwise.
	fn route_to_peer(&self, required_routing: RequiredRouting, peer: &PeerId) -> bool {
		match required_routing {
			RequiredRouting::All => true,
			RequiredRouting::GridX => self.peers_x.contains(peer),
			RequiredRouting::GridY => self.peers_y.contains(peer),
			RequiredRouting::GridXY => self.peers_x.contains(peer) || self.peers_y.contains(peer),
			RequiredRouting::None | RequiredRouting::PendingTopology => false,
		}
	}
}

impl From<network_bridge_event::NewGossipTopology> for SessionTopology {
	fn from(topology: network_bridge_event::NewGossipTopology) -> Self {
		let peers_x =
			topology.our_neighbors_x.values().flat_map(|p| &p.peer_ids).cloned().collect();
		let peers_y =
			topology.our_neighbors_y.values().flat_map(|p| &p.peer_ids).cloned().collect();

		let validator_indices_x =
			topology.our_neighbors_x.values().map(|p| p.validator_index.clone()).collect();
		let validator_indices_y =
			topology.our_neighbors_y.values().map(|p| p.validator_index.clone()).collect();

		SessionTopology { peers_x, peers_y, validator_indices_x, validator_indices_y }
	}
}

#[derive(Default, Clone)]
struct SessionTopologies {
	inner: HashMap<SessionIndex, (Option<SessionTopology>, usize)>,
}

impl SessionTopologies {
	fn get_topology(&self, session: SessionIndex) -> Option<SessionTopology> {
		self.inner.get(&session).and_then(|val| val.0.clone())
	}

	fn inc_session_refs(&mut self, session: SessionIndex) {
		self.inner.entry(session).or_insert((None, 0)).1 += 1;
	}

	fn dec_session_refs(&mut self, session: SessionIndex) {
		if let hash_map::Entry::Occupied(mut occupied) = self.inner.entry(session) {
			occupied.get_mut().1 = occupied.get().1.saturating_sub(1);
			if occupied.get().1 == 0 {
				let _ = occupied.remove();
			}
		}
	}

	// No-op if already present.
	fn insert_topology(&mut self, session: SessionIndex, topology: SessionTopology) {
		let entry = self.inner.entry(session).or_insert((None, 0));
		if entry.0.is_none() {
			entry.0 = Some(topology);
		}
	}
}

// A note on aggression thresholds: changes in propagation apply only to blocks which are the
// _direct descendants_ of the finalized block which are older than the given threshold,
// not to all blocks older than the threshold. Most likely, a few assignments struggle to
// be propagated in a single block and this holds up all of its descendants blocks.
// Accordingly, we only step on the gas for the block which is most obviously holding up finality.
#[derive(Clone)]
struct AggressionConfig {
	/// Aggression level 1: all validators send all their own messages to all peers.
	l1_threshold: Option<BlockNumber>,
	/// Aggression level 2: level 1 + all validators send all messages to all peers in the X and Y dimensions.
	l2_threshold: Option<BlockNumber>,
	/// How often to re-send messages to all targeted recipients.
	/// This applies to all unfinalized blocks.
	resend_unfinalized_period: Option<BlockNumber>,
}

impl AggressionConfig {
	fn is_age_relevant(&self, block_age: BlockNumber) -> bool {
		if let Some(t) = self.l1_threshold {
			block_age >= t
		} else if let Some(t) = self.resend_unfinalized_period {
			block_age > 0 && block_age % t == 0
		} else {
			false
		}
	}
}

impl Default for AggressionConfig {
	fn default() -> Self {
		AggressionConfig {
			l1_threshold: Some(10),
			l2_threshold: Some(25),
			resend_unfinalized_period: Some(5),
		}
	}
}

/// The [`State`] struct is responsible for tracking the overall state of the subsystem.
///
/// It tracks metadata about our view of the unfinalized chain,
/// which assignments and approvals we have seen, and our peers' views.
#[derive(Clone, Default)]
struct State {
	/// These two fields are used in conjunction to construct a view over the unfinalized chain.
	blocks_by_number: Arc<Mutex<BTreeMap<BlockNumber, Vec<Hash>>>>,
	blocks: Arc<DashMap<Hash, BlockEntry>>,

	/// Our view updates to our peers can race with `NewBlocks` updates. We store messages received
	/// against the directly mentioned blocks in our view in this map until `NewBlocks` is received.
	///
	/// As long as the parent is already in the `blocks` map and `NewBlocks` messages aren't delayed
	/// by more than a block length, this strategy will work well for mitigating the race. This is
	/// also a race that occurs typically on local networks.
	pending_known: Arc<DashMap<Hash, Vec<(PeerId, PendingMessage)>>>,

	/// Peer data is partially stored here, and partially inline within the [`BlockEntry`]s
	peer_views: Arc<DashMap<PeerId, View>>,

	/// Keeps a topology for various different sessions.
	topologies: Arc<Mutex<SessionTopologies>>,

	/// Tracks recently finalized blocks.
	recent_outdated_blocks: Arc<Mutex<RecentlyOutdated>>,

	/// Config for aggression.
	aggression_config: Arc<Mutex<AggressionConfig>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
	Assignment,
	Approval,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MessageSubject(Hash, CandidateIndex, ValidatorIndex);

#[derive(Debug, Clone, Default)]
struct Knowledge {
	// When there is no entry, this means the message is unknown
	// When there is an entry with `MessageKind::Assignment`, the assignment is known.
	// When there is an entry with `MessageKind::Approval`, the assignment and approval are known.
	known_messages: HashMap<MessageSubject, MessageKind>,
}

impl Knowledge {
	fn contains(&self, message: &MessageSubject, kind: MessageKind) -> bool {
		match (kind, self.known_messages.get(message)) {
			(_, None) => false,
			(MessageKind::Assignment, Some(_)) => true,
			(MessageKind::Approval, Some(MessageKind::Assignment)) => false,
			(MessageKind::Approval, Some(MessageKind::Approval)) => true,
		}
	}

	fn insert(&mut self, message: MessageSubject, kind: MessageKind) -> bool {
		match self.known_messages.entry(message) {
			hash_map::Entry::Vacant(vacant) => {
				vacant.insert(kind);
				true
			},
			hash_map::Entry::Occupied(mut occupied) => match (*occupied.get(), kind) {
				(MessageKind::Assignment, MessageKind::Assignment) => false,
				(MessageKind::Approval, MessageKind::Approval) => false,
				(MessageKind::Approval, MessageKind::Assignment) => false,
				(MessageKind::Assignment, MessageKind::Approval) => {
					*occupied.get_mut() = MessageKind::Approval;
					true
				},
			},
		}
	}
}

/// Information that has been circulated to and from a peer.
#[derive(Debug, Clone, Default)]
struct PeerKnowledge {
	/// The knowledge we've sent to the peer.
	sent: Knowledge,
	/// The knowledge we've received from the peer.
	received: Knowledge,
}

impl PeerKnowledge {
	fn contains(&self, message: &MessageSubject, kind: MessageKind) -> bool {
		self.sent.contains(message, kind) || self.received.contains(message, kind)
	}
}

/// Information about blocks in our current view as well as whether peers know of them.
struct BlockEntry {
	/// Peers who we know are aware of this block and thus, the candidates within it.
	/// This maps to their knowledge of messages.
	known_by: HashMap<PeerId, PeerKnowledge>,
	/// The number of the block.
	number: BlockNumber,
	/// The parent hash of the block.
	parent_hash: Hash,
	/// Our knowledge of messages.
	knowledge: Knowledge,
	/// A votes entry for each candidate indexed by [`CandidateIndex`].
	candidates: Vec<CandidateEntry>,
	/// The session index of this block.
	session: SessionIndex,
}

#[derive(Debug)]
enum ApprovalState {
	Assigned(AssignmentCert),
	Approved(AssignmentCert, ValidatorSignature),
}

impl ApprovalState {
	fn assignment_cert(&self) -> &AssignmentCert {
		match *self {
			ApprovalState::Assigned(ref cert) => cert,
			ApprovalState::Approved(ref cert, _) => cert,
		}
	}

	fn approval_signature(&self) -> Option<ValidatorSignature> {
		match *self {
			ApprovalState::Assigned(_) => None,
			ApprovalState::Approved(_, ref sig) => Some(sig.clone()),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RequiredRouting {
	/// We don't know yet, because we're waiting for topology info
	/// (race condition between learning about the first blocks in a new session
	/// and getting the topology for that session)
	PendingTopology,
	/// Propagate to all peers of any kind.
	All,
	/// Propagate to all peers sharing either the X or Y dimension of the grid.
	GridXY,
	/// Propagate to all peers sharing the X dimension of the grid.
	GridX,
	/// Propagate to all peers sharing the Y dimension of the grid.
	GridY,
	/// No required propagation.
	None,
}

impl RequiredRouting {
	// Whether the required routing set is definitely empty.
	fn is_empty(self) -> bool {
		match self {
			RequiredRouting::PendingTopology | RequiredRouting::None => true,
			_ => false,
		}
	}
}

#[derive(Debug, Default, Clone, Copy)]
struct RandomRouting {
	// The number of peers to target.
	target: usize,
	// The number of peers this has been sent to.
	sent: usize,
}

impl RandomRouting {
	fn sample(&self, n_peers_total: usize, rng: &mut (impl CryptoRng + Rng)) -> bool {
		if n_peers_total == 0 || self.sent >= self.target {
			false
		} else if RANDOM_SAMPLE_RATE > n_peers_total {
			true
		} else {
			rng.gen_ratio(RANDOM_SAMPLE_RATE as _, n_peers_total as _)
		}
	}

	fn inc_sent(&mut self) {
		self.sent += 1
	}
}

// routing state bundled with messages for the candidate. Corresponding assignments
// and approvals are stored together and should be routed in the same way, with
// assignments preceding approvals in all cases.
#[derive(Debug)]
struct MessageState {
	required_routing: RequiredRouting,
	local: bool,
	random_routing: RandomRouting,
	approval_state: ApprovalState,
}

/// Information about candidates in the context of a particular block they are included in.
/// In other words, multiple `CandidateEntry`s may exist for the same candidate,
/// if it is included by multiple blocks - this is likely the case when there are forks.
#[derive(Debug, Default)]
struct CandidateEntry {
	messages: HashMap<ValidatorIndex, MessageState>,
}

#[derive(Debug, Clone, PartialEq)]
enum MessageSource {
	Peer(PeerId),
	Local,
}

impl MessageSource {
	fn peer_id(&self) -> Option<PeerId> {
		match self {
			Self::Peer(id) => Some(id.clone()),
			Self::Local => None,
		}
	}
}

enum PendingMessage {
	Assignment(IndirectAssignmentCert, CandidateIndex),
	Approval(IndirectSignedApprovalVote),
}

impl State {
	async fn handle_network_msg(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		event: NetworkBridgeEvent<protocol_v1::ApprovalDistributionMessage>,
		rng: &mut (impl CryptoRng + Rng),
	) {
		match event {
			NetworkBridgeEvent::PeerConnected(peer_id, role, _) => {
				// insert a blank view if none already present
				gum::trace!(target: LOG_TARGET, ?peer_id, ?role, "Peer connected");
				self.peer_views.entry(peer_id).or_default();
			},
			NetworkBridgeEvent::PeerDisconnected(peer_id) => {
				gum::trace!(target: LOG_TARGET, ?peer_id, "Peer disconnected");
				self.peer_views.remove(&peer_id);
				self.blocks.iter_mut().for_each(|mut ref_multi| {
					ref_multi.value_mut().known_by.remove(&peer_id);
				})
			},
			NetworkBridgeEvent::NewGossipTopology(topology) => {
				let session = topology.session;
				self.handle_new_session_topology(
					ctx,
					metrics,
					session,
					SessionTopology::from(topology),
				)
				.await;
			},
			NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
				self.handle_peer_view_change(ctx, metrics, peer_id, view, rng).await;
			},
			NetworkBridgeEvent::OurViewChange(view) => {
				gum::trace!(target: LOG_TARGET, ?view, "Own view change");
				for head in view.iter() {
					if !self.blocks.contains_key(head) {
						self.pending_known.entry(*head).or_default();
					}
				}

				self.pending_known.retain(|h, _| {
					let live = view.contains(h);
					if !live {
						gum::trace!(
							target: LOG_TARGET,
							block_hash = ?h,
							"Cleaning up stale pending messages",
						);
					}
					live
				});
			},
			NetworkBridgeEvent::PeerMessage(peer_id, msg) => {
				self.process_incoming_peer_message(ctx, metrics, peer_id, msg, rng).await;
			},
		}
	}

	async fn handle_new_blocks(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		metas: Vec<BlockApprovalMeta>,
		rng: &mut (impl CryptoRng + Rng),
	) {
		let mut new_hashes = HashSet::new();
		for meta in &metas {
			match self.blocks.entry(meta.hash.clone()) {
				dashmap::mapref::entry::Entry::Vacant(entry) => {
					let candidates_count = meta.candidates.len();
					let mut candidates = Vec::with_capacity(candidates_count);
					candidates.resize_with(candidates_count, Default::default);

					entry.insert(BlockEntry {
						known_by: HashMap::new(),
						number: meta.number,
						parent_hash: meta.parent_hash.clone(),
						knowledge: Knowledge::default(),
						candidates,
						session: meta.session,
					});

					self.topologies.lock().expect("evil lock").inc_session_refs(meta.session);

					new_hashes.insert(meta.hash.clone());

					// In case there are duplicates, we should only set this if the entry
					// was vacant.
					self.blocks_by_number
						.lock()
						.expect("evil lock")
						.entry(meta.number)
						.or_default()
						.push(meta.hash);
				},
				_ => continue,
			}
		}

		gum::debug!(
			target: LOG_TARGET,
			"Got new blocks {:?}",
			metas.iter().map(|m| (m.hash, m.number)).collect::<Vec<_>>(),
		);

		{
			for ref_multi in self.peer_views.iter() {
				let (peer_id, view) = (ref_multi.key(), ref_multi.value());
				let intersection = view.iter().filter(|h| new_hashes.contains(h));
				let view_intersection = View::new(intersection.cloned(), view.finalized_number);
				Self::unify_with_peer(
					ctx,
					metrics,
					self.blocks.clone(),
					self.topologies.clone(),
					self.peer_views.len(),
					peer_id.clone(),
					view_intersection,
					rng,
				)
				.await;
			}

			let pending_now_known = self
				.pending_known
				.iter()
				.filter(|k| self.blocks.contains_key(k.key()))
				.map(|k| *k.key())
				.collect::<Vec<_>>();

			let to_import = pending_now_known
				.into_iter()
				.inspect(|h| {
					gum::trace!(
						target: LOG_TARGET,
						block_hash = ?h,
						"Extracting pending messages for new block"
					)
				})
				.filter_map(|k| self.pending_known.remove(&k).map(|entry| entry.1))
				.flatten()
				.collect::<Vec<_>>();

			if !to_import.is_empty() {
				gum::debug!(
					target: LOG_TARGET,
					num = to_import.len(),
					"Processing pending assignment/approvals",
				);

				let _timer = metrics.time_import_pending_now_known();

				for (peer_id, message) in to_import {
					match message {
						PendingMessage::Assignment(assignment, claimed_index) => {
							self.import_and_circulate_assignment(
								ctx,
								metrics,
								MessageSource::Peer(peer_id),
								assignment,
								claimed_index,
								rng,
							)
							.await;
						},
						PendingMessage::Approval(approval_vote) => {
							self.import_and_circulate_approval(
								ctx,
								metrics,
								MessageSource::Peer(peer_id),
								approval_vote,
							)
							.await;
						},
					}
				}
			}
		}

		// 'true' means trigger re-send of messages in old blocks.
		self.enable_aggression(ctx, true, metrics).await;
	}

	async fn handle_new_session_topology(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		session: SessionIndex,
		topology: SessionTopology,
	) {
		self.topologies.lock().expect("evil lock").insert_topology(session, topology);
		let topology = self
			.topologies
			.lock()
			.expect("evil lock")
			.get_topology(session)
			.expect("just inserted above; qed");

		let new_session_topology_stats = adjust_required_routing_and_propagate(
			ctx,
			self.blocks.clone(),
			self.topologies.clone(),
			|block_entry| block_entry.session == session,
			|required_routing, local, validator_index| {
				if *required_routing == RequiredRouting::PendingTopology {
					*required_routing = topology.required_routing_for(*validator_index, local);
				}
			},
		)
		.await;

		metrics.note_new_topology_stats(new_session_topology_stats);
	}

	async fn process_incoming_peer_message(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		peer_id: PeerId,
		msg: protocol_v1::ApprovalDistributionMessage,
		rng: &mut (impl CryptoRng + Rng),
	) {
		match msg {
			protocol_v1::ApprovalDistributionMessage::Assignments(assignments) => {
				gum::trace!(
					target: LOG_TARGET,
					peer_id = %peer_id,
					num = assignments.len(),
					"Processing assignments from a peer",
				);
				for (assignment, claimed_index) in assignments.into_iter() {
					if let Some(mut pending) = self.pending_known.get_mut(&assignment.block_hash) {
						let message_subject = MessageSubject(
							assignment.block_hash,
							claimed_index,
							assignment.validator,
						);

						gum::trace!(
							target: LOG_TARGET,
							%peer_id,
							?message_subject,
							"Pending assignment",
						);

						pending.push((
							peer_id.clone(),
							PendingMessage::Assignment(assignment, claimed_index),
						));

						continue
					}

					self.import_and_circulate_assignment(
						ctx,
						metrics,
						MessageSource::Peer(peer_id.clone()),
						assignment,
						claimed_index,
						rng,
					)
					.await;
				}
			},
			protocol_v1::ApprovalDistributionMessage::Approvals(approvals) => {
				gum::trace!(
					target: LOG_TARGET,
					peer_id = %peer_id,
					num = approvals.len(),
					"Processing approvals from a peer",
				);
				for approval_vote in approvals.into_iter() {
					if let Some(mut pending) = self.pending_known.get_mut(&approval_vote.block_hash)
					{
						let message_subject = MessageSubject(
							approval_vote.block_hash,
							approval_vote.candidate_index,
							approval_vote.validator,
						);

						gum::trace!(
							target: LOG_TARGET,
							%peer_id,
							?message_subject,
							"Pending approval",
						);

						pending.push((peer_id.clone(), PendingMessage::Approval(approval_vote)));

						continue
					}

					self.import_and_circulate_approval(
						ctx,
						metrics,
						MessageSource::Peer(peer_id.clone()),
						approval_vote,
					)
					.await;
				}
			},
		}
	}

	// handle a peer view change: requires that the peer is already connected
	// and has an entry in the `PeerData` struct.
	async fn handle_peer_view_change(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		peer_id: PeerId,
		view: View,
		rng: &mut (impl CryptoRng + Rng),
	) {
		gum::trace!(target: LOG_TARGET, ?view, "Peer view change");
		let finalized_number = view.finalized_number;
		let old_view = self
			.peer_views
			.get_mut(&peer_id)
			.map(|mut d| std::mem::replace(d.value_mut(), view.clone()));
		let old_finalized_number = old_view.map(|v| v.finalized_number).unwrap_or(0);

		// we want to prune every block known_by peer up to (including) view.finalized_number
		let blocks = &mut self.blocks;
		// the `BTreeMap::range` is constrained by stored keys
		// so the loop won't take ages if the new finalized_number skyrockets
		// but we need to make sure the range is not empty, otherwise it will panic
		// it shouldn't be, we make sure of this in the network bridge
		let range = old_finalized_number..=finalized_number;
		if !range.is_empty() && !blocks.is_empty() {
			self.blocks_by_number
				.lock()
				.expect("evil lock")
				.range(range)
				.flat_map(|(_number, hashes)| hashes)
				.for_each(|hash| {
					if let Some(mut entry) = blocks.get_mut(hash) {
						entry.known_by.remove(&peer_id);
					}
				});
		}

		Self::unify_with_peer(
			ctx,
			metrics,
			self.blocks.clone(),
			self.topologies.clone(),
			self.peer_views.len(),
			peer_id.clone(),
			view,
			rng,
		)
		.await;
	}

	async fn handle_block_finalized(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		finalized_number: BlockNumber,
	) {
		{
			// we want to prune every block up to (including) finalized_number
			// why +1 here?
			// split_off returns everything after the given key, including the key
			let split_point = finalized_number.saturating_add(1);
			let old_blocks =
				self.blocks_by_number.lock().expect("evil lock").split_off(&split_point);


			// after split_off old_blocks actually contains new blocks, we need to swap
			let old_value = std::mem::replace(&mut *self.blocks_by_number.lock().expect("evil lock"), old_blocks);
			*self.blocks_by_number.lock().expect("evil lock") = old_value;
			
			let old_blocks = self.blocks_by_number.lock().expect("evil lock");

			// now that we pruned `self.blocks_by_number`, let's clean up `self.blocks` too
			old_blocks.values().flatten().for_each(|relay_block| {
				self.recent_outdated_blocks
					.lock()
					.expect("evil lock")
					.note_outdated(*relay_block);
				if let Some(block_entry) = self.blocks.remove(relay_block) {
					self.topologies
						.lock()
						.expect("evil lock")
						.dec_session_refs(block_entry.1.session);
				}
			});
		}
		// If a block was finalized, this means we may need to move our aggression
		// forward to the now oldest block(s).
		// 'false' means don't trigger re-send of messages in old blocks.
		self.enable_aggression(ctx, false, metrics).await;
	}

	async fn import_and_circulate_assignment(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		source: MessageSource,
		assignment: IndirectAssignmentCert,
		claimed_candidate_index: CandidateIndex,
		rng: &mut (impl CryptoRng + Rng),
	) {
		let block_hash = assignment.block_hash.clone();
		let validator_index = assignment.validator;

		let mut entry = match self.blocks.get_mut(&block_hash) {
			Some(entry) => entry,
			None => {
				if let Some(peer_id) = source.peer_id() {
					gum::trace!(
						target: LOG_TARGET,
						?peer_id,
						hash = ?block_hash,
						?validator_index,
						"Unexpected assignment",
					);
					if !self
						.recent_outdated_blocks
						.lock()
						.expect("evil lock")
						.is_recent_outdated(&block_hash)
					{
						modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
					}
				}
				return
			},
		};

		// compute metadata on the assignment.
		let message_subject = MessageSubject(block_hash, claimed_candidate_index, validator_index);
		let message_kind = MessageKind::Assignment;

		if let Some(peer_id) = source.peer_id() {
			// check if our knowledge of the peer already contains this assignment
			match entry.known_by.entry(peer_id.clone()) {
				hash_map::Entry::Occupied(mut peer_knowledge) => {
					let peer_knowledge = peer_knowledge.get_mut();
					if peer_knowledge.contains(&message_subject, message_kind) {
						// wasn't included before
						if !peer_knowledge.received.insert(message_subject.clone(), message_kind) {
							gum::debug!(
								target: LOG_TARGET,
								?peer_id,
								?message_subject,
								"Duplicate assignment",
							);
							modify_reputation(ctx, peer_id, COST_DUPLICATE_MESSAGE).await;
						}
						return
					}
				},
				hash_map::Entry::Vacant(_) => {
					gum::debug!(
						target: LOG_TARGET,
						?peer_id,
						?message_subject,
						"Assignment from a peer is out of view",
					);
					modify_reputation(ctx, peer_id.clone(), COST_UNEXPECTED_MESSAGE).await;
				},
			}

			// if the assignment is known to be valid, reward the peer
			if entry.knowledge.contains(&message_subject, message_kind) {
				modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE).await;
				if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
					gum::trace!(target: LOG_TARGET, ?peer_id, ?message_subject, "Known assignment");
					peer_knowledge.received.insert(message_subject, message_kind);
				}
				return
			}

			let (tx, rx) = oneshot::channel();

			ctx.send_message(ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment.clone(),
				claimed_candidate_index,
				tx,
			)))
			.await;

			let timer = metrics.time_awaiting_approval_voting();
			let result = match rx.await {
				Ok(result) => result,
				Err(_) => {
					gum::debug!(target: LOG_TARGET, "The approval voting subsystem is down");
					return
				},
			};
			drop(timer);

			gum::trace!(
				target: LOG_TARGET,
				?source,
				?message_subject,
				?result,
				"Checked assignment",
			);
			match result {
				AssignmentCheckResult::Accepted => {
					modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;
					entry.knowledge.known_messages.insert(message_subject.clone(), message_kind);
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.received.insert(message_subject.clone(), message_kind);
					}
				},
				AssignmentCheckResult::AcceptedDuplicate => {
					// "duplicate" assignments aren't necessarily equal.
					// There is more than one way each validator can be assigned to each core.
					// cf. https://github.com/paritytech/polkadot/pull/2160#discussion_r557628699
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.received.insert(message_subject.clone(), message_kind);
					}
					gum::debug!(
						target: LOG_TARGET,
						hash = ?block_hash,
						?peer_id,
						"Got an `AcceptedDuplicate` assignment",
					);
					return
				},
				AssignmentCheckResult::TooFarInFuture => {
					gum::debug!(
						target: LOG_TARGET,
						hash = ?block_hash,
						?peer_id,
						"Got an assignment too far in the future",
					);
					modify_reputation(ctx, peer_id, COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE).await;
					return
				},
				AssignmentCheckResult::Bad(error) => {
					gum::info!(
						target: LOG_TARGET,
						hash = ?block_hash,
						?peer_id,
						%error,
						"Got a bad assignment from peer",
					);
					modify_reputation(ctx, peer_id, COST_INVALID_MESSAGE).await;
					return
				},
			}
		} else {
			if !entry.knowledge.insert(message_subject.clone(), message_kind) {
				// if we already imported an assignment, there is no need to distribute it again
				gum::warn!(
					target: LOG_TARGET,
					?message_subject,
					"Importing locally an already known assignment",
				);
				return
			} else {
				gum::debug!(
					target: LOG_TARGET,
					?message_subject,
					"Importing locally a new assignment",
				);
			}
		}

		// Invariant: to our knowledge, none of the peers except for the `source` know about the assignment.
		metrics.on_assignment_imported();

		let topology = self.topologies.lock().expect("evil lock").get_topology(entry.session);
		let local = source == MessageSource::Local;

		let required_routing = topology.as_ref().map_or(RequiredRouting::PendingTopology, |t| {
			t.required_routing_for(validator_index, local)
		});

		
		let message_state = match entry.candidates.get_mut(claimed_candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Assigned
				// unless the approval state is set already
				candidate_entry.messages.entry(validator_index).or_insert_with(|| MessageState {
					required_routing,
					local,
					random_routing: RandomRouting { target: RANDOM_CIRCULATION, sent: 0 },
					approval_state: ApprovalState::Assigned(assignment.cert.clone()),
				})
			},
			None => {
				gum::warn!(
					target: LOG_TARGET,
					hash = ?block_hash,
					?claimed_candidate_index,
					"Expected a candidate entry on import_and_circulate_assignment",
				);

				return
			},
		};

		// Dispatch the message to all peers in the routing set which
		// know the block.
		//
		// If the topology isn't known yet (race with networking subsystems)
		// then messages will be sent when we get it.

		let assignments = vec![(assignment, claimed_candidate_index)];
		let n_peers_total = self.peer_views.len();
		let source_peer = source.peer_id();

		let topology_ref = topology.as_ref();
		let mut peer_filter = move |peer| {
			if Some(peer) == source_peer.as_ref() {
				return false
			}

			if let Some(true) = topology_ref.map(|t| t.route_to_peer(required_routing, peer)) {
				return true
			}

			// Note: at this point, we haven't received the message from any peers
			// other than the source peer, and we just got it, so we haven't sent it
			// to any peers either.
			let route_random = message_state.random_routing.sample(n_peers_total, rng);

			if route_random {
				message_state.random_routing.inc_sent();
			}

			route_random
		};

		let mut entry = self.blocks.get_mut(&block_hash).unwrap();

		let peers = entry.known_by.keys().filter(|p| peer_filter(p)).cloned().collect::<Vec<_>>();

		// Add the metadata of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(peer_knowledge) = entry.known_by.get_mut(peer) {
				peer_knowledge.sent.insert(message_subject.clone(), message_kind);
			}
		}

		let mut stats = SentMessagesStats::default();

		if !peers.is_empty() {
			gum::trace!(
				target: LOG_TARGET,
				?block_hash,
				?claimed_candidate_index,
				local = source.peer_id().is_none(),
				num_peers = peers.len(),
				"Sending an assignment to peers",
			);

			stats.assignments += peers.len();
			stats.assignment_packets += peers.len();
			ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments),
				),
			)))
			.await;
		}

		metrics.note_basic_circulation_stats(stats);
	}

	async fn import_and_circulate_approval(
		&mut self,
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		source: MessageSource,
		vote: IndirectSignedApprovalVote,
	) {
		let block_hash = vote.block_hash.clone();
		let validator_index = vote.validator;
		let candidate_index = vote.candidate_index;

		let mut entry = match self.blocks.get_mut(&block_hash) {
			Some(entry) if entry.candidates.get(candidate_index as usize).is_some() => entry,
			_ => {
				if let Some(peer_id) = source.peer_id() {
					if !self
						.recent_outdated_blocks
						.lock()
						.expect("evil lock")
						.is_recent_outdated(&block_hash)
					{
						modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
					}
				}
				return
			},
		};

		// compute metadata on the assignment.
		let message_subject = MessageSubject(block_hash, candidate_index, validator_index);
		let message_kind = MessageKind::Approval;

		if let Some(peer_id) = source.peer_id() {
			if !entry.knowledge.contains(&message_subject, MessageKind::Assignment) {
				gum::debug!(
					target: LOG_TARGET,
					?peer_id,
					?message_subject,
					"Unknown approval assignment",
				);
				modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			// check if our knowledge of the peer already contains this approval
			match entry.known_by.entry(peer_id.clone()) {
				hash_map::Entry::Occupied(mut knowledge) => {
					let peer_knowledge = knowledge.get_mut();
					if peer_knowledge.contains(&message_subject, message_kind) {
						if !peer_knowledge.received.insert(message_subject.clone(), message_kind) {
							gum::debug!(
								target: LOG_TARGET,
								?peer_id,
								?message_subject,
								"Duplicate approval",
							);

							modify_reputation(ctx, peer_id, COST_DUPLICATE_MESSAGE).await;
						}
						return
					}
				},
				hash_map::Entry::Vacant(_) => {
					gum::debug!(
						target: LOG_TARGET,
						?peer_id,
						?message_subject,
						"Approval from a peer is out of view",
					);
					modify_reputation(ctx, peer_id.clone(), COST_UNEXPECTED_MESSAGE).await;
				},
			}

			// if the approval is known to be valid, reward the peer
			if entry.knowledge.contains(&message_subject, message_kind) {
				gum::trace!(target: LOG_TARGET, ?peer_id, ?message_subject, "Known approval");
				modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE).await;
				if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
					peer_knowledge.received.insert(message_subject.clone(), message_kind);
				}
				return
			}

			let (tx, rx) = oneshot::channel();

			let timer = metrics.time_awaiting_approval_voting();

			ctx.send_message(ApprovalVoting(ApprovalVotingMessage::CheckAndImportApproval(
				vote.clone(),
				tx,
			)))
			.await;

			let result = match rx.await {
				Ok(result) => result,
				Err(_) => {
					gum::debug!(target: LOG_TARGET, "The approval voting subsystem is down");
					return
				},
			};
			drop(timer);

			gum::trace!(
				target: LOG_TARGET,
				?peer_id,
				?message_subject,
				?result,
				"Checked approval",
			);
			match result {
				ApprovalCheckResult::Accepted => {
					modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;

					entry.knowledge.insert(message_subject.clone(), message_kind);
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.received.insert(message_subject.clone(), message_kind);
					}
				},
				ApprovalCheckResult::Bad(error) => {
					modify_reputation(ctx, peer_id, COST_INVALID_MESSAGE).await;
					gum::info!(
						target: LOG_TARGET,
						?peer_id,
						%error,
						"Got a bad approval from peer",
					);
					return
				},
			}
		} else {
			if !entry.knowledge.insert(message_subject.clone(), message_kind) {
				// if we already imported an approval, there is no need to distribute it again
				gum::warn!(
					target: LOG_TARGET,
					?message_subject,
					"Importing locally an already known approval",
				);
				return
			} else {
				gum::debug!(
					target: LOG_TARGET,
					?message_subject,
					"Importing locally a new approval",
				);
			}
		}

		// Invariant: to our knowledge, none of the peers except for the `source` know about the approval.
		metrics.on_approval_imported();

		let required_routing = match entry.candidates.get_mut(candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Approved
				// it should be in assigned state already
				match candidate_entry.messages.remove(&validator_index) {
					Some(MessageState {
						approval_state: ApprovalState::Assigned(cert),
						required_routing,
						local,
						random_routing,
					}) => {
						candidate_entry.messages.insert(
							validator_index,
							MessageState {
								approval_state: ApprovalState::Approved(
									cert,
									vote.signature.clone(),
								),
								required_routing,
								local,
								random_routing,
							},
						);

						required_routing
					},
					Some(_) => {
						unreachable!(
							"we only insert it after the metadata, checked the metadata above; qed"
						);
					},
					None => {
						// this would indicate a bug in approval-voting
						gum::warn!(
							target: LOG_TARGET,
							hash = ?block_hash,
							?candidate_index,
							?validator_index,
							"Importing an approval we don't have an assignment for",
						);

						return
					},
				}
			},
			None => {
				gum::warn!(
					target: LOG_TARGET,
					hash = ?block_hash,
					?candidate_index,
					?validator_index,
					"Expected a candidate entry on import_and_circulate_approval",
				);

				return
			},
		};

		// Dispatch a ApprovalDistributionV1Message::Approval(vote)
		// to all peers required by the topology, with the exception of the source peer.

		let topology = self.topologies.lock().expect("evil lock").get_topology(entry.session);
		let source_peer = source.peer_id();

		let message_subject = &message_subject;
	
		let peers = entry
			.known_by
			.iter()
			.filter(move |(peer, knowledge)| {
				if Some(*peer) == source_peer.as_ref() {
					return false
				}
	
				// Here we're leaning on a few behaviors of assignment propagation:
				//   1. At this point, the only peer we're aware of which has the approval
				//      message is the source peer.
				//   2. We have sent the assignment message to every peer in the required routing
				//      which is aware of this block _unless_ the peer we originally received the
				//      assignment from was part of the required routing. In that case, we've sent
				//      the assignment to all aware peers in the required routing _except_ the original
				//      source of the assignment. Hence the `in_topology_check`.
				//   3. Any randomly selected peers have been sent the assignment already.
				let in_topology = topology.as_ref().map_or(false, |t| t.route_to_peer(required_routing, peer));
				in_topology || knowledge.sent.contains(message_subject, MessageKind::Assignment)
			})
			.map(|(p, _)| p)
			.cloned()
			.collect::<Vec<_>>();

		// Add the metadata of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(entry) = entry.known_by.get_mut(peer) {
				entry.sent.insert(message_subject.clone(), message_kind);
			}
		}

		let mut stats = SentMessagesStats::default();

		if !peers.is_empty() {
			let approvals = vec![vote];
			gum::trace!(
				target: LOG_TARGET,
				?block_hash,
				?candidate_index,
				local = source.peer_id().is_none(),
				num_peers = peers.len(),
				"Sending an approval to peers",
			);

			stats.approvals += peers.len();
			stats.approval_packets += peers.len();
			ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals),
				),
			)))
			.await;
		}

		metrics.note_basic_circulation_stats(stats);
	}

	async fn unify_with_peer(
		ctx: &mut impl SubsystemSender,
		metrics: &Metrics,
		entries: Arc<DashMap<Hash, BlockEntry>>,
		topologies: Arc<Mutex<SessionTopologies>>,
		total_peers: usize,
		peer_id: PeerId,
		view: View,
		rng: &mut (impl CryptoRng + Rng),
	) {
		metrics.on_unify_with_peer();
		let _timer = metrics.time_unify_with_peer();

		let mut assignments_to_send = Vec::new();
		let mut approvals_to_send = Vec::new();

		let view_finalized_number = view.finalized_number;
		for head in view.into_iter() {
			let mut block = head;
			loop {
				// Gonna hold this lock for a while. A dashmap would be better.
				match entries.get_mut(&block) {
					Some(mut entry) if entry.number > view_finalized_number => {
						// entry.value_mut(),
						// Any peer which is in the `known_by` set has already been
						// sent all messages it's meant to get for that block and all
						// in-scope prior blocks.
						if entry.value().known_by.contains_key(&peer_id) {
							break
						}
						let entry = entry.value_mut();

						let peer_knowledge = entry.known_by.entry(peer_id.clone()).or_default();

						let candidates =
							entry.candidates.iter_mut().enumerate().flat_map(|(c_i, c)| {
								c.messages.iter_mut().map(move |(k, v)| (c_i as _, k, v))
							});
						// Iterate all messages in all candidates.
						for (candidate_index, validator, message_state) in candidates {
							// Propagate the message to all peers in the required routing set OR
							// randomly sample peers.
							{
								let topology = topologies
									.lock()
									.expect("evil lock")
									.get_topology(entry.session);

								let random_routing = &mut message_state.random_routing;
								let required_routing = message_state.required_routing;
								let rng = &mut *rng;
								let mut peer_filter = move |peer_id| {
									let in_topology = topology.as_ref().map_or(false, |t| {
										t.route_to_peer(required_routing, peer_id)
									});
									in_topology || {
										let route_random = random_routing.sample(total_peers, rng);
										if route_random {
											random_routing.inc_sent();
										}

										route_random
									}
								};

								if !peer_filter(&peer_id) {
									continue
								}
							}

							let message_subject =
								MessageSubject(block.clone(), candidate_index, validator.clone());

							let assignment_message = (
								IndirectAssignmentCert {
									block_hash: block.clone(),
									validator: validator.clone(),
									cert: message_state.approval_state.assignment_cert().clone(),
								},
								candidate_index,
							);

							let approval_message = message_state
								.approval_state
								.approval_signature()
								.map(|signature| IndirectSignedApprovalVote {
									block_hash: block.clone(),
									validator: validator.clone(),
									candidate_index,
									signature,
								});

							if !peer_knowledge.contains(&message_subject, MessageKind::Assignment) {
								peer_knowledge
									.sent
									.insert(message_subject.clone(), MessageKind::Assignment);
								assignments_to_send.push(assignment_message);
							}

							if let Some(approval_message) = approval_message {
								if !peer_knowledge.contains(&message_subject, MessageKind::Approval)
								{
									peer_knowledge
										.sent
										.insert(message_subject.clone(), MessageKind::Approval);
									approvals_to_send.push(approval_message);
								}
							}
						}

						block = entry.parent_hash.clone();
					},
					_ => break,
				};
			}
		}

		let mut stats = SentMessagesStats::default();

		if !assignments_to_send.is_empty() {
			stats.note_assignments_packet(assignments_to_send.len());
			ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id.clone()],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments_to_send),
				),
			)))
			.await;
		}

		if !approvals_to_send.is_empty() {
			stats.note_approvals_packet(approvals_to_send.len());
			ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id.clone()],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals_to_send),
				),
			)))
			.await;
		}

		metrics.note_unify_with_peer_stats(stats);
	}

	async fn enable_aggression(
		&mut self,
		ctx: &mut impl SubsystemSender,
		do_resend: bool,
		metrics: &Metrics,
	) {
		let min_age = self
			.blocks_by_number
			.lock()
			.expect("evil lock")
			.iter()
			.next()
			.map(|(num, _)| num)
			.cloned();
		let max_age = self
			.blocks_by_number
			.lock()
			.expect("evil lock")
			.iter()
			.rev()
			.next()
			.map(|(num, _)| num)
			.cloned();
		let config = self.aggression_config.clone();

		let (min_age, max_age) = match (min_age, max_age) {
			(Some(min), Some(max)) => (min, max),
			_ => return, // empty.
		};

		let diff = max_age - min_age;
		if !self.aggression_config.lock().expect("evil lock").is_age_relevant(diff) {
			return
		}

		let resend_stats = adjust_required_routing_and_propagate(
			ctx,
			self.blocks.clone(),
			self.topologies.clone(),
			|block_entry| {
				let block_age = max_age - block_entry.number;

				if do_resend &&
					config
						.lock()
						.expect("evil lock")
						.resend_unfinalized_period
						.as_ref()
						.map_or(false, |p| block_age > 0 && block_age % p == 0)
				{
					// Retry sending to all peers.
					for (_, knowledge) in block_entry.known_by.iter_mut() {
						knowledge.sent = Default::default();
					}

					true
				} else {
					false
				}
			},
			|_, _, _| {},
		)
		.await;

		let aggression_stats = adjust_required_routing_and_propagate(
			ctx,
			self.blocks.clone(),
			self.topologies.clone(),
			|block_entry| {
				// Ramp up aggression only for the very oldest block(s).
				// Approval voting can get stuck on a single block preventing
				// its descendants from being finalized. Waste minimal bandwidth
				// this way. Also, disputes might prevent finality - again, nothing
				// to waste bandwidth on newer blocks for.
				block_entry.number == min_age
			},
			|required_routing, local, _| {
				// It's a bit surprising not to have a topology at this age.
				if *required_routing == RequiredRouting::PendingTopology {
					gum::debug!(
						target: LOG_TARGET,
						age = ?diff,
						"Encountered old block pending gossip topology",
					);
					return
				}

				if config
					.lock()
					.expect("evil lock")
					.l1_threshold
					.as_ref()
					.map_or(false, |t| &diff >= t)
				{
					// Message originator sends to everyone.
					if local && *required_routing != RequiredRouting::All {
						metrics.on_aggression_l1();
						*required_routing = RequiredRouting::All;
					}
				}

				if config
					.lock()
					.expect("evil lock")
					.l2_threshold
					.as_ref()
					.map_or(false, |t| &diff >= t)
				{
					// Message originator sends to everyone. Everyone else sends to XY.
					if !local && *required_routing != RequiredRouting::GridXY {
						metrics.on_aggression_l2();
						*required_routing = RequiredRouting::GridXY;
					}
				}
			},
		)
		.await;

		metrics.note_resend_stats(resend_stats);
		metrics.note_aggression_stats(aggression_stats);
	}
}

#[derive(Default)]
struct SentMessagesStats {
	assignments: usize,
	approvals: usize,
	assignment_packets: usize,
	approval_packets: usize,
}

impl SentMessagesStats {
	fn note_assignments_packet(&mut self, assignments: usize) {
		self.assignment_packets += 1;
		self.assignments += assignments;
	}

	fn note_approvals_packet(&mut self, approvals: usize) {
		self.approval_packets += 1;
		self.approvals += approvals;
	}
}

// This adjusts the required routing of messages in blocks that pass the block filter
// according to the modifier function given.
//
// The modifier accepts as inputs the current required-routing state, whether
// the message is locally originating, and the validator index of the message issuer.
//
// Then, if the topology is known, this progates messages to all peers in the required
// routing set which are aware of the block. Peers which are unaware of the block
// will have the message sent when it enters their view in `unify_with_peer`.
//
// Note that the required routing of a message can be modified even if the
// topology is unknown yet.
async fn adjust_required_routing_and_propagate(
	ctx: &mut impl SubsystemSender,
	blocks: Arc<DashMap<Hash, BlockEntry>>,
	topologies: Arc<Mutex<SessionTopologies>>,
	block_filter: impl Fn(&mut BlockEntry) -> bool,
	routing_modifier: impl Fn(&mut RequiredRouting, bool, &ValidatorIndex),
) -> SentMessagesStats {
	let mut stats = SentMessagesStats::default();

	let mut peer_assignments = HashMap::new();
	let mut peer_approvals = HashMap::new();

	// Iterate all blocks in the session, producing payloads
	// for each connected peer.
	for mut ref_multi in blocks.iter_mut() {
		let (block_hash, block_entry) = ref_multi.pair_mut();

		if !block_filter(block_entry) {
			continue
		}

		// Iterate all messages in all candidates.
		for (candidate_index, validator, message_state) in block_entry
			.candidates
			.iter_mut()
			.enumerate()
			.flat_map(|(c_i, c)| c.messages.iter_mut().map(move |(k, v)| (c_i as _, k, v)))
		{
			routing_modifier(&mut message_state.required_routing, message_state.local, validator);

			if message_state.required_routing.is_empty() {
				continue
			}

			let topology =
				match topologies.lock().expect("evil lock").get_topology(block_entry.session) {
					Some(t) => t,
					None => continue,
				};

			// Propagate the message to all peers in the required routing set.
			let message_subject =
				MessageSubject(block_hash.clone(), candidate_index, validator.clone());

			let assignment_message = (
				IndirectAssignmentCert {
					block_hash: block_hash.clone(),
					validator: validator.clone(),
					cert: message_state.approval_state.assignment_cert().clone(),
				},
				candidate_index,
			);
			let approval_message =
				message_state.approval_state.approval_signature().map(|signature| {
					IndirectSignedApprovalVote {
						block_hash: block_hash.clone(),
						validator: validator.clone(),
						candidate_index,
						signature,
					}
				});

			for (peer, peer_knowledge) in &mut block_entry.known_by {
				if !topology.route_to_peer(message_state.required_routing, peer) {
					continue
				}

				if !peer_knowledge.contains(&message_subject, MessageKind::Assignment) {
					peer_knowledge.sent.insert(message_subject.clone(), MessageKind::Assignment);
					peer_assignments
						.entry(peer.clone())
						.or_insert_with(Vec::new)
						.push(assignment_message.clone());
				}

				if let Some(approval_message) = approval_message.as_ref() {
					if !peer_knowledge.contains(&message_subject, MessageKind::Approval) {
						peer_knowledge.sent.insert(message_subject.clone(), MessageKind::Approval);
						peer_approvals
							.entry(peer.clone())
							.or_insert_with(Vec::new)
							.push(approval_message.clone());
					}
				}
			}
		}
	}

	// Send messages in accumulated packets, assignments preceding approvals.

	for (peer, assignments_packet) in peer_assignments {
		stats.note_assignments_packet(assignments_packet.len());
		ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
			vec![peer],
			protocol_v1::ValidationProtocol::ApprovalDistribution(
				protocol_v1::ApprovalDistributionMessage::Assignments(assignments_packet),
			),
		)))
		.await;
	}

	for (peer, approvals_packet) in peer_approvals {
		stats.note_approvals_packet(approvals_packet.len());
		ctx.send_message(NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
			vec![peer],
			protocol_v1::ValidationProtocol::ApprovalDistribution(
				protocol_v1::ApprovalDistributionMessage::Approvals(approvals_packet),
			),
		)))
		.await;
	}

	stats
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(ctx: &mut impl SubsystemSender, peer_id: PeerId, rep: Rep) {
	gum::trace!(
		target: LOG_TARGET,
		reputation = ?rep,
		?peer_id,
		"Reputation change for peer",
	);

	ctx.send_message(NetworkBridge(NetworkBridgeMessage::ReportPeer(peer_id, rep)))
		.await;
}

impl ApprovalDistributionWorker {
	/// Create a new instance of the [`ApprovalDistribution`] subsystem.
	pub fn new(
		metrics: Metrics,
		proxy_receiver: futures::channel::mpsc::Receiver<FromOverseer<ApprovalDistributionMessage>>,
	) -> Self {
		Self { metrics, proxy_receiver }
	}

	async fn run(self, ctx: impl SubsystemSender, state: State) {
		// According to the docs of `rand`, this is a ChaCha12 RNG in practice
		// and will always be chosen for strong performance and security properties.
		let mut rng = rand::rngs::StdRng::from_entropy();
		self.run_inner(ctx, state, &mut rng).await;
	}

	/// Used for testing.
	async fn run_inner(
		mut self,
		mut ctx: impl SubsystemSender,
		mut state: State,
		rng: &mut (impl CryptoRng + Rng),
	) {
		loop {
			let message = match self.proxy_receiver.next().await {
				Some(message) => message,
				None => {
					gum::debug!(target: LOG_TARGET, "Failed to receive a message, exiting");
					return
				},
			};
			match message {
				FromOverseer::Communication { msg } =>
					Self::handle_incoming(&mut ctx, state.clone(), msg, &self.metrics, rng).await,
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					..
				})) => {
					gum::trace!(target: LOG_TARGET, "active leaves signal (ignored)");
					// the relay chain blocks relevant to the approval subsystems
					// are those that are available, but not finalized yet
					// actived and deactivated heads hence are irrelevant to this subsystem
				},
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number)) => {
					gum::trace!(target: LOG_TARGET, number = %number, "finalized signal");
					state.handle_block_finalized(&mut ctx, &self.metrics, number).await;
				},
				FromOverseer::Signal(OverseerSignal::Conclude) => return,
			}
		}
	}

	async fn handle_incoming(
		ctx: &mut impl SubsystemSender,
		mut state: State,
		msg: ApprovalDistributionMessage,
		metrics: &Metrics,
		rng: &mut (impl CryptoRng + Rng),
	) {
		match msg {
			ApprovalDistributionMessage::NetworkBridgeUpdateV1(event) => {
				state.handle_network_msg(ctx, metrics, event, rng).await;
			},
			ApprovalDistributionMessage::NewBlocks(metas) => {
				state.handle_new_blocks(ctx, metrics, metas, rng).await;
			},
			ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index) => {
				gum::debug!(
					target: LOG_TARGET,
					"Distributing our assignment on candidate (block={}, index={})",
					cert.block_hash,
					candidate_index,
				);

				state
					.import_and_circulate_assignment(
						ctx,
						&metrics,
						MessageSource::Local,
						cert,
						candidate_index,
						rng,
					)
					.await;
			},
			ApprovalDistributionMessage::DistributeApproval(vote) => {
				gum::debug!(
					target: LOG_TARGET,
					"Distributing our approval vote on candidate (block={}, index={})",
					vote.block_hash,
					vote.candidate_index,
				);

				state
					.import_and_circulate_approval(ctx, metrics, MessageSource::Local, vote)
					.await;
			},
		}
	}
}

impl<Context> overseer::Subsystem<Context, SubsystemError> for ApprovalDistribution
where
	Context: SubsystemContext<Message = ApprovalDistributionMessage>,
	Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self.run(ctx).map(|_| Ok(())).boxed();

		SpawnedSubsystem { name: "approval-distribution-subsystem", future }
	}
}

impl ApprovalDistribution {
	/// Create a new instance of the [`ApprovalDistribution`] subsystem. This is just a wrapper
	/// which distributes work to single threaded `ApprovalDistributionWorker` instances in a pool.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	async fn run<Context>(self, mut ctx: Context)
	where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
		Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		let mut pool = Vec::new();
		let state = State::default();

		// Spawn our workers.
		for _ in 0..WORKER_COUNT {
			// Make the proxy pipes.
			let (proxy_sender, proxy_receiver) = futures::channel::mpsc::channel(1024);
			let subsystem_sender = ctx.sender().clone();
			let worker = ApprovalDistributionWorker::new(self.metrics.clone(), proxy_receiver);
			ctx.spawn(
				"approval-distribution-worker",
				Box::pin(worker.run(subsystem_sender, state.clone())),
			)
			.unwrap();
			pool.push(proxy_sender);
		}

		self.run_inner(ctx, pool).await
	}

	/// Used for testing.
	async fn run_inner<Context>(
		self,
		mut ctx: Context,
		mut pool: Vec<futures::channel::mpsc::Sender<FromOverseer<ApprovalDistributionMessage>>>,
	) where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
		Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		let mut index = 0usize;
		loop {
			// We need to compute the worker shard index.
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					gum::debug!(target: LOG_TARGET, err = ?e, "Failed to receive a message from Overseer, exiting");
					return
				},
			};

			match pool[index % WORKER_COUNT as usize].send(message).await {
				Ok(_) => {},
				Err(err) => {
					gum::debug!(target: LOG_TARGET, ?err, "Failed to send a message to a worker");
				},
			}

			index += 1;
		}
	}
}
