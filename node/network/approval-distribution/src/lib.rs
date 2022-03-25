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

use futures::{channel::oneshot, FutureExt as _};
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
	SubsystemError,
};
use polkadot_primitives::v2::{
	BlockNumber, CandidateIndex, Hash, SessionIndex, ValidatorIndex, ValidatorSignature,
};
use std::collections::{hash_map, BTreeMap, HashMap, HashSet, VecDeque};

use self::metrics::Metrics;

mod metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::approval-distribution";

const COST_UNEXPECTED_MESSAGE: Rep =
	Rep::CostMinor("Peer sent an out-of-view assignment or approval");
const COST_DUPLICATE_MESSAGE: Rep = Rep::CostMinorRepeated("Peer sent identical messages");
const COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE: Rep =
	Rep::CostMinor("The vote was valid but too far in the future");
const COST_INVALID_MESSAGE: Rep = Rep::CostMajor("The vote was bad");

const BENEFIT_VALID_MESSAGE: Rep = Rep::BenefitMinor("Peer sent a valid message");
const BENEFIT_VALID_MESSAGE_FIRST: Rep =
	Rep::BenefitMinorFirst("Valid message with new information");

/// The Approval Distribution subsystem.
pub struct ApprovalDistribution {
	metrics: Metrics,
}

#[derive(Default)]
struct PeerData {
	view: View,
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

struct SessionTopology {
	peers_x: HashSet<PeerId>,
	validator_indices_x: HashSet<ValidatorIndex>,
	peers_y: HashSet<PeerId>,
	validator_indices_y: HashSet<ValidatorIndex>,
}

impl SessionTopology {
	fn required_routing_for(&self, validator_index: ValidatorIndex) -> RequiredRouting {
		let grid_x = self.validator_indices_x.contains(&validator_index);
		let grid_y = self.validator_indices_y.contains(&validator_index);

		match (grid_x, grid_y) {
			(false, false) => RequiredRouting::None,
			(true, false) => RequiredRouting::GridX,
			(false, true) => RequiredRouting::GridY,
			(true, true) => RequiredRouting::GridXY, // if the grid works as expected, this shouldn't happen.
		}
	}

	// Get a filter function based on this topology and the required routing
	// which returns `true` for peers that are within the required routing set
	// and false otherwise.
	fn peer_filter<'a>(
		&'a self,
		required_routing: RequiredRouting,
	) -> impl Fn(&PeerId) -> bool + 'a {
		let (grid_x, grid_y) = match required_routing {
			RequiredRouting::GridX => (true, false),
			RequiredRouting::GridY => (false, true),
			RequiredRouting::GridXY => (true, true),
			RequiredRouting::None | RequiredRouting::PendingTopology => (false, false),
		};

		move |peer| {
			(grid_x && self.peers_x.contains(peer)) || (grid_y && self.peers_y.contains(peer))
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

#[derive(Default)]
struct SessionTopologies {
	inner: HashMap<SessionIndex, (Option<SessionTopology>, usize)>,
}

impl SessionTopologies {
	fn get_topology(&self, session: SessionIndex) -> Option<&SessionTopology> {
		self.inner.get(&session).and_then(|val| val.0.as_ref())
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

/// The [`State`] struct is responsible for tracking the overall state of the subsystem.
///
/// It tracks metadata about our view of the unfinalized chain,
/// which assignments and approvals we have seen, and our peers' views.
#[derive(Default)]
struct State {
	/// These two fields are used in conjunction to construct a view over the unfinalized chain.
	blocks_by_number: BTreeMap<BlockNumber, Vec<Hash>>,
	blocks: HashMap<Hash, BlockEntry>,

	/// Our view updates to our peers can race with `NewBlocks` updates. We store messages received
	/// against the directly mentioned blocks in our view in this map until `NewBlocks` is received.
	///
	/// As long as the parent is already in the `blocks` map and `NewBlocks` messages aren't delayed
	/// by more than a block length, this strategy will work well for mitigating the race. This is
	/// also a race that occurs typically on local networks.
	pending_known: HashMap<Hash, Vec<(PeerId, PendingMessage)>>,

	/// Peer data is partially stored here, and partially inline within the [`BlockEntry`]s
	peer_data: HashMap<PeerId, PeerData>,

	/// Topologies for various different sessions.
	topologies: SessionTopologies,

	/// Tracks recently finalized blocks.
	recent_outdated_blocks: RecentlyOutdated,
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
	/// Propagate to all peers sharing either the X or Y dimension of the grid.
	GridXY,
	/// Propagate to all peers sharing the X dimension of the grid.
	GridX,
	/// Propagate to all peers sharing the Y dimension of the grid.
	GridY,
	/// No required progation.
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

// routing state bundled with messages for the candidate. Corresponding assignments
// and approvals are stored together and should be routed in the same way, with
// assignments preceding approvals in all cases.
#[derive(Debug)]
struct MessageState {
	required_routing: RequiredRouting,
	random_routing: usize, // Number peers to target in random routing.
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
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		event: NetworkBridgeEvent<protocol_v1::ApprovalDistributionMessage>,
	) {
		match event {
			NetworkBridgeEvent::PeerConnected(peer_id, role, _) => {
				// insert a blank view if none already present
				gum::trace!(target: LOG_TARGET, ?peer_id, ?role, "Peer connected");
				self.peer_data.entry(peer_id).or_default();
			},
			NetworkBridgeEvent::PeerDisconnected(peer_id) => {
				gum::trace!(target: LOG_TARGET, ?peer_id, "Peer disconnected");
				self.peer_data.remove(&peer_id);
				self.blocks.iter_mut().for_each(|(_hash, entry)| {
					entry.known_by.remove(&peer_id);
				})
			},
			NetworkBridgeEvent::NewGossipTopology(topology) => {
				let session = topology.session;
				self.handle_new_session_topology(ctx, session, SessionTopology::from(topology))
					.await;
			},
			NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
				self.handle_peer_view_change(ctx, metrics, peer_id, view).await;
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
				self.process_incoming_peer_message(ctx, metrics, peer_id, msg).await;
			},
		}
	}

	async fn handle_new_blocks(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		metas: Vec<BlockApprovalMeta>,
	) {
		let mut new_hashes = HashSet::new();
		for meta in &metas {
			match self.blocks.entry(meta.hash.clone()) {
				hash_map::Entry::Vacant(entry) => {
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

					self.topologies.inc_session_refs(meta.session);

					new_hashes.insert(meta.hash.clone());

					// In case there are duplicates, we should only set this if the entry
					// was vacant.
					self.blocks_by_number.entry(meta.number).or_default().push(meta.hash);
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
			for (peer_id, peer_data) in self.peer_data.iter() {
				let intersection = peer_data.view.iter().filter(|h| new_hashes.contains(h));
				let view_intersection =
					View::new(intersection.cloned(), peer_data.view.finalized_number);
				Self::unify_with_peer(
					ctx,
					metrics,
					&mut self.blocks,
					&self.topologies,
					peer_id.clone(),
					view_intersection,
				)
				.await;
			}

			let pending_now_known = self
				.pending_known
				.keys()
				.filter(|k| self.blocks.contains_key(k))
				.copied()
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
				.filter_map(|k| self.pending_known.remove(&k))
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
	}

	async fn handle_new_session_topology(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		session: SessionIndex,
		topology: SessionTopology,
	) {
		self.topologies.insert_topology(session, topology);
		let topology = self.topologies.get_topology(session).expect("just inserted above; qed");

		let mut peer_assignments = HashMap::new();
		let mut peer_approvals = HashMap::new();

		// Iterate all blocks in the session, producing payloads
		// for each connected peer.
		for (block_hash, block_entry) in &mut self.blocks {
			if block_entry.session != session {
				continue
			}

			// Iterate all messages in all candidates.
			for (candidate_index, validator, message_state) in block_entry
				.candidates
				.iter_mut()
				.enumerate()
				.flat_map(|(c_i, c)| c.messages.iter_mut().map(move |(k, v)| (c_i as _, k, v)))
			{
				if message_state.required_routing == RequiredRouting::PendingTopology {
					message_state.required_routing =
						topology.required_routing_for(validator.clone());
				}

				if message_state.required_routing.is_empty() {
					continue
				}

				// Propagate the message to all peers in the required routing set.
				let peer_filter = topology.peer_filter(message_state.required_routing);
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
					if !peer_filter(peer) {
						continue
					}

					if !peer_knowledge.contains(&message_subject, MessageKind::Assignment) {
						peer_knowledge
							.sent
							.insert(message_subject.clone(), MessageKind::Assignment);
						peer_assignments
							.entry(peer.clone())
							.or_insert_with(Vec::new)
							.push(assignment_message.clone());
					}

					if let Some(approval_message) = approval_message.as_ref() {
						if !peer_knowledge.contains(&message_subject, MessageKind::Approval) {
							peer_knowledge
								.sent
								.insert(message_subject.clone(), MessageKind::Approval);
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
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments_packet),
				),
			))
			.await;
		}

		for (peer, approvals_packet) in peer_approvals {
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals_packet),
				),
			))
			.await;
		}
	}

	async fn process_incoming_peer_message(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		peer_id: PeerId,
		msg: protocol_v1::ApprovalDistributionMessage,
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
					if let Some(pending) = self.pending_known.get_mut(&assignment.block_hash) {
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
					if let Some(pending) = self.pending_known.get_mut(&approval_vote.block_hash) {
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
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		peer_id: PeerId,
		view: View,
	) {
		gum::trace!(target: LOG_TARGET, ?view, "Peer view change");
		let finalized_number = view.finalized_number;
		let old_view = self
			.peer_data
			.get_mut(&peer_id)
			.map(|d| std::mem::replace(&mut d.view, view.clone()));
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
				.range(range)
				.flat_map(|(_number, hashes)| hashes)
				.for_each(|hash| {
					if let Some(entry) = blocks.get_mut(hash) {
						entry.known_by.remove(&peer_id);
					}
				});
		}

		Self::unify_with_peer(
			ctx,
			metrics,
			&mut self.blocks,
			&self.topologies,
			peer_id.clone(),
			view,
		)
		.await;
	}

	fn handle_block_finalized(&mut self, finalized_number: BlockNumber) {
		// we want to prune every block up to (including) finalized_number
		// why +1 here?
		// split_off returns everything after the given key, including the key
		let split_point = finalized_number.saturating_add(1);
		let mut old_blocks = self.blocks_by_number.split_off(&split_point);

		// after split_off old_blocks actually contains new blocks, we need to swap
		std::mem::swap(&mut self.blocks_by_number, &mut old_blocks);

		// now that we pruned `self.blocks_by_number`, let's clean up `self.blocks` too
		old_blocks.values().flatten().for_each(|relay_block| {
			self.recent_outdated_blocks.note_outdated(*relay_block);
			if let Some(block_entry) = self.blocks.remove(relay_block) {
				self.topologies.dec_session_refs(block_entry.session);
			}
		});
	}

	async fn import_and_circulate_assignment(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		source: MessageSource,
		assignment: IndirectAssignmentCert,
		claimed_candidate_index: CandidateIndex,
	) {
		let block_hash = assignment.block_hash.clone();
		let validator_index = assignment.validator;

		let entry = match self.blocks.get_mut(&block_hash) {
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
					if !self.recent_outdated_blocks.is_recent_outdated(&block_hash) {
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

			ctx.send_message(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment.clone(),
				claimed_candidate_index,
				tx,
			))
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

		let topology = self.topologies.get_topology(entry.session);

		let required_routing = if source == MessageSource::Local {
			RequiredRouting::GridXY
		} else {
			topology.map_or(RequiredRouting::PendingTopology, |t| {
				t.required_routing_for(validator_index)
			})
		};

		match entry.candidates.get_mut(claimed_candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Assigned
				// unless the approval state is set already
				candidate_entry.messages.entry(validator_index).or_insert_with(|| {
					// TODO [now]: do random routing.
					MessageState {
						required_routing,
						random_routing: 0,
						approval_state: ApprovalState::Assigned(assignment.cert.clone()),
					}
				});
			},
			None => {
				gum::warn!(
					target: LOG_TARGET,
					hash = ?block_hash,
					?claimed_candidate_index,
					"Expected a candidate entry on import_and_circulate_assignment",
				);
			},
		}

		let topology = match topology {
			None => return,
			Some(t) => t,
		};

		if required_routing.is_empty() {
			return
		}

		// Dispatch the message to all peers in the routing set which
		// know the block.
		//
		// If the topology isn't known yet (race with networking subsystems)
		// then messages will be sent when we get it.

		let assignments = vec![(assignment, claimed_candidate_index)];
		let topology_filter = topology.peer_filter(required_routing);
		let source_peer = source.peer_id();

		let peers = entry
			.known_by
			.keys()
			.filter(|p| topology_filter(p))
			.filter(|p| source_peer.as_ref().map_or(true, |source| &source != p))
			.cloned()
			.collect::<Vec<_>>();

		// Add the metadata of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(peer_knowledge) = entry.known_by.get_mut(peer) {
				peer_knowledge.sent.insert(message_subject.clone(), message_kind);
			}
		}

		if !peers.is_empty() {
			gum::trace!(
				target: LOG_TARGET,
				?block_hash,
				?claimed_candidate_index,
				local = source.peer_id().is_none(),
				num_peers = peers.len(),
				"Sending an assignment to peers",
			);

			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments),
				),
			))
			.await;
		}
	}

	async fn import_and_circulate_approval(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		source: MessageSource,
		vote: IndirectSignedApprovalVote,
	) {
		let block_hash = vote.block_hash.clone();
		let validator_index = vote.validator;
		let candidate_index = vote.candidate_index;

		let entry = match self.blocks.get_mut(&block_hash) {
			Some(entry) if entry.candidates.get(candidate_index as usize).is_some() => entry,
			_ => {
				if let Some(peer_id) = source.peer_id() {
					if !self.recent_outdated_blocks.is_recent_outdated(&block_hash) {
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

			ctx.send_message(ApprovalVotingMessage::CheckAndImportApproval(vote.clone(), tx))
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

		let topology = match self.topologies.get_topology(entry.session) {
			Some(t) => t,
			None => return,
		};

		if required_routing.is_empty() {
			return
		}

		let topology_filter = topology.peer_filter(required_routing);
		let source_peer = source.peer_id();
		let peers = entry
			.known_by
			.keys()
			.filter(|p| topology_filter(p))
			.filter(|p| source_peer.as_ref().map_or(true, |source| &source != p))
			.cloned()
			.collect::<Vec<_>>();

		// Add the metadata of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(entry) = entry.known_by.get_mut(peer) {
				entry.sent.insert(message_subject.clone(), message_kind);
			}
		}

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

			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals),
				),
			))
			.await;
		}
	}

	async fn unify_with_peer(
		ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
		          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
		metrics: &Metrics,
		entries: &mut HashMap<Hash, BlockEntry>,
		topologies: &SessionTopologies,
		peer_id: PeerId,
		view: View,
	) {
		metrics.on_unify_with_peer();
		let _timer = metrics.time_unify_with_peer();

		let mut assignments_to_send = Vec::new();
		let mut approvals_to_send = Vec::new();

		let view_finalized_number = view.finalized_number;
		for head in view.into_iter() {
			let mut block = head;
			loop {
				let sent_before = assignments_to_send.len() + approvals_to_send.len();
				let entry = match entries.get_mut(&block) {
					Some(entry) if entry.number > view_finalized_number => entry,
					_ => break,
				};

				// Any peer which is in the `known_by` set has already been
				// sent all messages it's meant to get for that block and all
				// in-scope prior blocks.
				if entry.known_by.contains_key(&peer_id) {
					break
				}

				let peer_knowledge = entry.known_by.entry(peer_id.clone()).or_default();

				let topology = match topologies.get_topology(entry.session) {
					Some(t) => t,
					None => {
						// The gossip topology for a recently entered session might be missing
						// as we're still awaiting it from the network subsystems.
						//
						// We'll send required messages when we get it.

						block = entry.parent_hash.clone();
						continue
					},
				};

				// Iterate all messages in all candidates.
				for (candidate_index, validator, message_state) in
					entry.candidates.iter_mut().enumerate().flat_map(|(c_i, c)| {
						c.messages.iter_mut().map(move |(k, v)| (c_i as _, k, v))
					}) {
					if message_state.required_routing.is_empty() {
						continue
					}

					// Propagate the message to all peers in the required routing set.
					let peer_filter = topology.peer_filter(message_state.required_routing);
					if !peer_filter(&peer_id) {
						continue
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

					let approval_message =
						message_state.approval_state.approval_signature().map(|signature| {
							IndirectSignedApprovalVote {
								block_hash: block.clone(),
								validator: validator.clone(),
								candidate_index,
								signature,
							}
						});

					if !peer_knowledge.contains(&message_subject, MessageKind::Assignment) {
						peer_knowledge
							.sent
							.insert(message_subject.clone(), MessageKind::Assignment);
						assignments_to_send.push(assignment_message);
					}

					if let Some(approval_message) = approval_message {
						if !peer_knowledge.contains(&message_subject, MessageKind::Approval) {
							peer_knowledge
								.sent
								.insert(message_subject.clone(), MessageKind::Approval);
							approvals_to_send.push(approval_message);
						}
					}
				}

				block = entry.parent_hash.clone();
			}
		}

		if !assignments_to_send.is_empty() {
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id.clone()],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments_to_send),
				),
			))
			.await;
		}

		if !approvals_to_send.is_empty() {
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id.clone()],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals_to_send),
				),
			))
			.await;
		}
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
	ctx: &mut (impl SubsystemContext<Message = ApprovalDistributionMessage>
	          + overseer::SubsystemContext<Message = ApprovalDistributionMessage>),
	peer_id: PeerId,
	rep: Rep,
) {
	gum::trace!(
		target: LOG_TARGET,
		reputation = ?rep,
		?peer_id,
		"Reputation change for peer",
	);

	ctx.send_message(NetworkBridgeMessage::ReportPeer(peer_id, rep)).await;
}

impl ApprovalDistribution {
	/// Create a new instance of the [`ApprovalDistribution`] subsystem.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	async fn run<Context>(self, ctx: Context)
	where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
		Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		let mut state = State::default();
		self.run_inner(ctx, &mut state).await
	}

	/// Used for testing.
	async fn run_inner<Context>(self, mut ctx: Context, state: &mut State)
	where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
		Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		loop {
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					gum::debug!(target: LOG_TARGET, err = ?e, "Failed to receive a message from Overseer, exiting");
					return
				},
			};
			match message {
				FromOverseer::Communication { msg } =>
					Self::handle_incoming(&mut ctx, state, msg, &self.metrics).await,
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
					state.handle_block_finalized(number);
				},
				FromOverseer::Signal(OverseerSignal::Conclude) => return,
			}
		}
	}

	async fn handle_incoming<Context>(
		ctx: &mut Context,
		state: &mut State,
		msg: ApprovalDistributionMessage,
		metrics: &Metrics,
	) where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
		Context: overseer::SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		match msg {
			ApprovalDistributionMessage::NetworkBridgeUpdateV1(event) => {
				state.handle_network_msg(ctx, metrics, event).await;
			},
			ApprovalDistributionMessage::NewBlocks(metas) => {
				state.handle_new_blocks(ctx, metrics, metas).await;
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
