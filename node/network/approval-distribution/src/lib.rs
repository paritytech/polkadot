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
	self as net_protocol,
	grid_topology::{RandomRouting, RequiredRouting, SessionGridTopologies, SessionGridTopology},
	peer_set::MAX_NOTIFICATION_SIZE,
	v1 as protocol_v1, PeerId, UnifiedReputationChange as Rep, Versioned, View,
};
use polkadot_node_primitives::approval::{
	AssignmentCert, BlockApprovalMeta, IndirectAssignmentCert, IndirectSignedApprovalVote,
};
use polkadot_node_subsystem::{
	messages::{
		ApprovalCheckResult, ApprovalDistributionMessage, ApprovalVotingMessage,
		AssignmentCheckResult, NetworkBridgeEvent, NetworkBridgeTxMessage,
	},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_primitives::v2::{
	BlockNumber, CandidateIndex, Hash, SessionIndex, ValidatorIndex, ValidatorSignature,
};
use rand::{CryptoRng, Rng, SeedableRng};
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

// In case the original gtid topology mechanisms don't work on their own, we need to trade bandwidth
// for protocol liveliness by introducing aggression.
//
// Aggression has 3 levels:
//
//  * Aggression Level 0: The basic behaviors described above.
//  * Aggression Level 1: The originator of a message sends to all peers. Other peers follow the rules above.
//  * Aggression Level 2: All peers send all messages to all their row and column neighbors.
//    This means that each validator will, on average, receive each message approximately `2*sqrt(n)` times.
// The aggression level of messages pertaining to a block increases when that block is unfinalized and
// is a child of the finalized block.
// This means that only one block at a time has its messages propagated with aggression > 0.
//
// A note on aggression thresholds: changes in propagation apply only to blocks which are the
// _direct descendants_ of the finalized block which are older than the given threshold,
// not to all blocks older than the threshold. Most likely, a few assignments struggle to
// be propagated in a single block and this holds up all of its descendants blocks.
// Accordingly, we only step on the gas for the block which is most obviously holding up finality.

/// Aggression configuration representation
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
	/// Returns `true` if block is not too old depending on the aggression level
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
			l1_threshold: Some(13),
			l2_threshold: Some(28),
			resend_unfinalized_period: Some(8),
		}
	}
}

#[derive(PartialEq)]
enum Resend {
	Yes,
	No,
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
	peer_views: HashMap<PeerId, View>,

	/// Keeps a topology for various different sessions.
	topologies: SessionGridTopologies,

	/// Tracks recently finalized blocks.
	recent_outdated_blocks: RecentlyOutdated,

	/// Config for aggression.
	aggression_config: AggressionConfig,
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
			Self::Peer(id) => Some(*id),
			Self::Local => None,
		}
	}
}

enum PendingMessage {
	Assignment(IndirectAssignmentCert, CandidateIndex),
	Approval(IndirectSignedApprovalVote),
}

#[overseer::contextbounds(ApprovalDistribution, prefix = self::overseer)]
impl State {
	async fn handle_network_msg<Context>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		event: NetworkBridgeEvent<net_protocol::ApprovalDistributionMessage>,
		rng: &mut (impl CryptoRng + Rng),
	) {
		match event {
			NetworkBridgeEvent::PeerConnected(peer_id, role, _, _) => {
				// insert a blank view if none already present
				gum::trace!(target: LOG_TARGET, ?peer_id, ?role, "Peer connected");
				self.peer_views.entry(peer_id).or_default();
			},
			NetworkBridgeEvent::PeerDisconnected(peer_id) => {
				gum::trace!(target: LOG_TARGET, ?peer_id, "Peer disconnected");
				self.peer_views.remove(&peer_id);
				self.blocks.iter_mut().for_each(|(_hash, entry)| {
					entry.known_by.remove(&peer_id);
				})
			},
			NetworkBridgeEvent::NewGossipTopology(topology) => {
				self.handle_new_session_topology(
					ctx,
					topology.session,
					topology.topology,
					topology.local_index,
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
			NetworkBridgeEvent::PeerMessage(peer_id, Versioned::V1(msg)) => {
				self.process_incoming_peer_message(ctx, metrics, peer_id, msg, rng).await;
			},
		}
	}

	async fn handle_new_blocks<Context>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		metas: Vec<BlockApprovalMeta>,
		rng: &mut (impl CryptoRng + Rng),
	) {
		let mut new_hashes = HashSet::new();
		for meta in &metas {
			match self.blocks.entry(meta.hash) {
				hash_map::Entry::Vacant(entry) => {
					let candidates_count = meta.candidates.len();
					let mut candidates = Vec::with_capacity(candidates_count);
					candidates.resize_with(candidates_count, Default::default);

					entry.insert(BlockEntry {
						known_by: HashMap::new(),
						number: meta.number,
						parent_hash: meta.parent_hash,
						knowledge: Knowledge::default(),
						candidates,
						session: meta.session,
					});

					self.topologies.inc_session_refs(meta.session);

					new_hashes.insert(meta.hash);

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
			let sender = ctx.sender();
			for (peer_id, view) in self.peer_views.iter() {
				let intersection = view.iter().filter(|h| new_hashes.contains(h));
				let view_intersection = View::new(intersection.cloned(), view.finalized_number);
				Self::unify_with_peer(
					sender,
					metrics,
					&mut self.blocks,
					&self.topologies,
					self.peer_views.len(),
					*peer_id,
					view_intersection,
					rng,
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

		self.enable_aggression(ctx, Resend::Yes, metrics).await;
	}

	async fn handle_new_session_topology<Context>(
		&mut self,
		ctx: &mut Context,
		session: SessionIndex,
		topology: SessionGridTopology,
		local_index: Option<ValidatorIndex>,
	) {
		if local_index.is_none() {
			// this subsystem only matters to validators.
			return
		}

		self.topologies.insert_topology(session, topology, local_index);
		let topology = self.topologies.get_topology(session).expect("just inserted above; qed");

		adjust_required_routing_and_propagate(
			ctx,
			&mut self.blocks,
			&self.topologies,
			|block_entry| block_entry.session == session,
			|required_routing, local, validator_index| {
				if *required_routing == RequiredRouting::PendingTopology {
					*required_routing = topology
						.local_grid_neighbors()
						.required_routing_by_index(*validator_index, local);
				}
			},
		)
		.await;
	}

	async fn process_incoming_peer_message<Context, R>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		peer_id: PeerId,
		msg: protocol_v1::ApprovalDistributionMessage,
		rng: &mut R,
	) where
		R: CryptoRng + Rng,
	{
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

						pending
							.push((peer_id, PendingMessage::Assignment(assignment, claimed_index)));

						continue
					}

					self.import_and_circulate_assignment(
						ctx,
						metrics,
						MessageSource::Peer(peer_id),
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

						pending.push((peer_id, PendingMessage::Approval(approval_vote)));

						continue
					}

					self.import_and_circulate_approval(
						ctx,
						metrics,
						MessageSource::Peer(peer_id),
						approval_vote,
					)
					.await;
				}
			},
		}
	}

	// handle a peer view change: requires that the peer is already connected
	// and has an entry in the `PeerData` struct.
	async fn handle_peer_view_change<Context, R>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		peer_id: PeerId,
		view: View,
		rng: &mut R,
	) where
		R: CryptoRng + Rng,
	{
		gum::trace!(target: LOG_TARGET, ?view, "Peer view change");
		let finalized_number = view.finalized_number;
		let old_view =
			self.peer_views.get_mut(&peer_id).map(|d| std::mem::replace(d, view.clone()));
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
			ctx.sender(),
			metrics,
			&mut self.blocks,
			&self.topologies,
			self.peer_views.len(),
			peer_id,
			view,
			rng,
		)
		.await;
	}

	async fn handle_block_finalized<Context>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		finalized_number: BlockNumber,
	) {
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

		// If a block was finalized, this means we may need to move our aggression
		// forward to the now oldest block(s).
		self.enable_aggression(ctx, Resend::No, metrics).await;
	}

	async fn import_and_circulate_assignment<Context, R>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		source: MessageSource,
		assignment: IndirectAssignmentCert,
		claimed_candidate_index: CandidateIndex,
		rng: &mut R,
	) where
		R: CryptoRng + Rng,
	{
		let block_hash = assignment.block_hash;
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
						modify_reputation(ctx.sender(), peer_id, COST_UNEXPECTED_MESSAGE).await;
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
			match entry.known_by.entry(peer_id) {
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
							modify_reputation(ctx.sender(), peer_id, COST_DUPLICATE_MESSAGE).await;
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
					modify_reputation(ctx.sender(), peer_id, COST_UNEXPECTED_MESSAGE).await;
				},
			}

			// if the assignment is known to be valid, reward the peer
			if entry.knowledge.contains(&message_subject, message_kind) {
				modify_reputation(ctx.sender(), peer_id, BENEFIT_VALID_MESSAGE).await;
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
					modify_reputation(ctx.sender(), peer_id, BENEFIT_VALID_MESSAGE_FIRST).await;
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
					modify_reputation(ctx.sender(), peer_id, COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE)
						.await;
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
					modify_reputation(ctx.sender(), peer_id, COST_INVALID_MESSAGE).await;
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
		let local = source == MessageSource::Local;

		let required_routing = topology.map_or(RequiredRouting::PendingTopology, |t| {
			t.local_grid_neighbors().required_routing_by_index(validator_index, local)
		});

		let message_state = match entry.candidates.get_mut(claimed_candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Assigned
				// unless the approval state is set already
				candidate_entry.messages.entry(validator_index).or_insert_with(|| MessageState {
					required_routing,
					local,
					random_routing: Default::default(),
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

		let mut peer_filter = move |peer| {
			if Some(peer) == source_peer.as_ref() {
				return false
			}

			if let Some(true) = topology
				.as_ref()
				.map(|t| t.local_grid_neighbors().route_to_peer(required_routing, peer))
			{
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

		let peers = entry.known_by.keys().filter(|p| peer_filter(p)).cloned().collect::<Vec<_>>();

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

			ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments),
				)),
			))
			.await;
		}
	}

	async fn import_and_circulate_approval<Context>(
		&mut self,
		ctx: &mut Context,
		metrics: &Metrics,
		source: MessageSource,
		vote: IndirectSignedApprovalVote,
	) {
		let block_hash = vote.block_hash;
		let validator_index = vote.validator;
		let candidate_index = vote.candidate_index;

		let entry = match self.blocks.get_mut(&block_hash) {
			Some(entry) if entry.candidates.get(candidate_index as usize).is_some() => entry,
			_ => {
				if let Some(peer_id) = source.peer_id() {
					if !self.recent_outdated_blocks.is_recent_outdated(&block_hash) {
						modify_reputation(ctx.sender(), peer_id, COST_UNEXPECTED_MESSAGE).await;
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
				modify_reputation(ctx.sender(), peer_id, COST_UNEXPECTED_MESSAGE).await;
				return
			}

			// check if our knowledge of the peer already contains this approval
			match entry.known_by.entry(peer_id) {
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

							modify_reputation(ctx.sender(), peer_id, COST_DUPLICATE_MESSAGE).await;
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
					modify_reputation(ctx.sender(), peer_id, COST_UNEXPECTED_MESSAGE).await;
				},
			}

			// if the approval is known to be valid, reward the peer
			if entry.knowledge.contains(&message_subject, message_kind) {
				gum::trace!(target: LOG_TARGET, ?peer_id, ?message_subject, "Known approval");
				modify_reputation(ctx.sender(), peer_id, BENEFIT_VALID_MESSAGE).await;
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
					modify_reputation(ctx.sender(), peer_id, BENEFIT_VALID_MESSAGE_FIRST).await;

					entry.knowledge.insert(message_subject.clone(), message_kind);
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.received.insert(message_subject.clone(), message_kind);
					}
				},
				ApprovalCheckResult::Bad(error) => {
					modify_reputation(ctx.sender(), peer_id, COST_INVALID_MESSAGE).await;
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

		let topology = self.topologies.get_topology(entry.session);
		let source_peer = source.peer_id();

		let message_subject = &message_subject;
		let peer_filter = move |peer, knowledge: &PeerKnowledge| {
			if Some(peer) == source_peer.as_ref() {
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
			let in_topology = topology
				.map_or(false, |t| t.local_grid_neighbors().route_to_peer(required_routing, peer));
			in_topology || knowledge.sent.contains(message_subject, MessageKind::Assignment)
		};

		let peers = entry
			.known_by
			.iter()
			.filter(|(p, k)| peer_filter(p, k))
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

			ctx.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals),
				)),
			))
			.await;
		}
	}

	/// Retrieve approval signatures from state for the given relay block/indices:
	fn get_approval_signatures(
		&mut self,
		indices: HashSet<(Hash, CandidateIndex)>,
	) -> HashMap<ValidatorIndex, ValidatorSignature> {
		let mut all_sigs = HashMap::new();
		for (hash, index) in indices {
			let block_entry = match self.blocks.get(&hash) {
				None => {
					gum::debug!(
						target: LOG_TARGET,
						?hash,
						"`get_approval_signatures`: could not find block entry for given hash!"
					);
					continue
				},
				Some(e) => e,
			};

			let candidate_entry = match block_entry.candidates.get(index as usize) {
				None => {
					gum::debug!(
						target: LOG_TARGET,
						?hash,
						?index,
						"`get_approval_signatures`: could not find candidate entry for given hash and index!"
						);
					continue
				},
				Some(e) => e,
			};
			let sigs =
				candidate_entry.messages.iter().filter_map(|(validator_index, message_state)| {
					match &message_state.approval_state {
						ApprovalState::Approved(_, sig) => Some((*validator_index, sig.clone())),
						ApprovalState::Assigned(_) => None,
					}
				});
			all_sigs.extend(sigs);
		}
		all_sigs
	}

	async fn unify_with_peer(
		sender: &mut impl overseer::ApprovalDistributionSenderTrait,
		metrics: &Metrics,
		entries: &mut HashMap<Hash, BlockEntry>,
		topologies: &SessionGridTopologies,
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

				let peer_knowledge = entry.known_by.entry(peer_id).or_default();

				let topology = topologies.get_topology(entry.session);

				// Iterate all messages in all candidates.
				for (candidate_index, validator, message_state) in
					entry.candidates.iter_mut().enumerate().flat_map(|(c_i, c)| {
						c.messages.iter_mut().map(move |(k, v)| (c_i as _, k, v))
					}) {
					// Propagate the message to all peers in the required routing set OR
					// randomly sample peers.
					{
						let random_routing = &mut message_state.random_routing;
						let required_routing = message_state.required_routing;
						let rng = &mut *rng;
						let mut peer_filter = move |peer_id| {
							let in_topology = topology.as_ref().map_or(false, |t| {
								t.local_grid_neighbors().route_to_peer(required_routing, peer_id)
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

					let message_subject = MessageSubject(block, candidate_index, *validator);

					let assignment_message = (
						IndirectAssignmentCert {
							block_hash: block,
							validator: *validator,
							cert: message_state.approval_state.assignment_cert().clone(),
						},
						candidate_index,
					);

					let approval_message =
						message_state.approval_state.approval_signature().map(|signature| {
							IndirectSignedApprovalVote {
								block_hash: block,
								validator: *validator,
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

				block = entry.parent_hash;
			}
		}

		if !assignments_to_send.is_empty() {
			gum::trace!(
				target: LOG_TARGET,
				?peer_id,
				num = assignments_to_send.len(),
				"Sending assignments to unified peer",
			);

			send_assignments_batched(sender, assignments_to_send, peer_id).await;
		}

		if !approvals_to_send.is_empty() {
			gum::trace!(
				target: LOG_TARGET,
				?peer_id,
				num = approvals_to_send.len(),
				"Sending approvals to unified peer",
			);

			send_approvals_batched(sender, approvals_to_send, peer_id).await;
		}
	}

	async fn enable_aggression<Context>(
		&mut self,
		ctx: &mut Context,
		resend: Resend,
		metrics: &Metrics,
	) {
		let min_age = self.blocks_by_number.iter().next().map(|(num, _)| num);
		let max_age = self.blocks_by_number.iter().rev().next().map(|(num, _)| num);
		let config = self.aggression_config.clone();

		let (min_age, max_age) = match (min_age, max_age) {
			(Some(min), Some(max)) => (min, max),
			_ => return, // empty.
		};

		let diff = max_age - min_age;
		if !self.aggression_config.is_age_relevant(diff) {
			return
		}

		adjust_required_routing_and_propagate(
			ctx,
			&mut self.blocks,
			&self.topologies,
			|block_entry| {
				let block_age = max_age - block_entry.number;

				if resend == Resend::Yes &&
					config
						.resend_unfinalized_period
						.as_ref()
						.map_or(false, |p| block_age > 0 && block_age % p == 0)
				{
					// Retry sending to all peers.
					for (_, knowledge) in block_entry.known_by.iter_mut() {
						knowledge.sent = Knowledge::default();
					}

					true
				} else {
					false
				}
			},
			|_, _, _| {},
		)
		.await;

		adjust_required_routing_and_propagate(
			ctx,
			&mut self.blocks,
			&self.topologies,
			|block_entry| {
				// Ramp up aggression only for the very oldest block(s).
				// Approval voting can get stuck on a single block preventing
				// its descendants from being finalized. Waste minimal bandwidth
				// this way. Also, disputes might prevent finality - again, nothing
				// to waste bandwidth on newer blocks for.
				&block_entry.number == min_age
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

				if config.l1_threshold.as_ref().map_or(false, |t| &diff >= t) {
					// Message originator sends to everyone.
					if local && *required_routing != RequiredRouting::All {
						metrics.on_aggression_l1();
						*required_routing = RequiredRouting::All;
					}
				}

				if config.l2_threshold.as_ref().map_or(false, |t| &diff >= t) {
					// Message originator sends to everyone. Everyone else sends to XY.
					if !local && *required_routing != RequiredRouting::GridXY {
						metrics.on_aggression_l2();
						*required_routing = RequiredRouting::GridXY;
					}
				}
			},
		)
		.await;
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
#[overseer::contextbounds(ApprovalDistribution, prefix = self::overseer)]
async fn adjust_required_routing_and_propagate<Context, BlockFilter, RoutingModifier>(
	ctx: &mut Context,
	blocks: &mut HashMap<Hash, BlockEntry>,
	topologies: &SessionGridTopologies,
	block_filter: BlockFilter,
	routing_modifier: RoutingModifier,
) where
	BlockFilter: Fn(&mut BlockEntry) -> bool,
	RoutingModifier: Fn(&mut RequiredRouting, bool, &ValidatorIndex),
{
	let mut peer_assignments = HashMap::new();
	let mut peer_approvals = HashMap::new();

	// Iterate all blocks in the session, producing payloads
	// for each connected peer.
	for (block_hash, block_entry) in blocks {
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

			let topology = match topologies.get_topology(block_entry.session) {
				Some(t) => t,
				None => continue,
			};

			// Propagate the message to all peers in the required routing set.
			let message_subject = MessageSubject(*block_hash, candidate_index, *validator);

			let assignment_message = (
				IndirectAssignmentCert {
					block_hash: *block_hash,
					validator: *validator,
					cert: message_state.approval_state.assignment_cert().clone(),
				},
				candidate_index,
			);
			let approval_message =
				message_state.approval_state.approval_signature().map(|signature| {
					IndirectSignedApprovalVote {
						block_hash: *block_hash,
						validator: *validator,
						candidate_index,
						signature,
					}
				});

			for (peer, peer_knowledge) in &mut block_entry.known_by {
				if !topology
					.local_grid_neighbors()
					.route_to_peer(message_state.required_routing, peer)
				{
					continue
				}

				if !peer_knowledge.contains(&message_subject, MessageKind::Assignment) {
					peer_knowledge.sent.insert(message_subject.clone(), MessageKind::Assignment);
					peer_assignments
						.entry(*peer)
						.or_insert_with(Vec::new)
						.push(assignment_message.clone());
				}

				if let Some(approval_message) = approval_message.as_ref() {
					if !peer_knowledge.contains(&message_subject, MessageKind::Approval) {
						peer_knowledge.sent.insert(message_subject.clone(), MessageKind::Approval);
						peer_approvals
							.entry(*peer)
							.or_insert_with(Vec::new)
							.push(approval_message.clone());
					}
				}
			}
		}
	}

	// Send messages in accumulated packets, assignments preceding approvals.

	for (peer, assignments_packet) in peer_assignments {
		send_assignments_batched(ctx.sender(), assignments_packet, peer).await;
	}

	for (peer, approvals_packet) in peer_approvals {
		send_approvals_batched(ctx.sender(), approvals_packet, peer).await;
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
	sender: &mut impl overseer::ApprovalDistributionSenderTrait,
	peer_id: PeerId,
	rep: Rep,
) {
	gum::trace!(
		target: LOG_TARGET,
		reputation = ?rep,
		?peer_id,
		"Reputation change for peer",
	);

	sender.send_message(NetworkBridgeTxMessage::ReportPeer(peer_id, rep)).await;
}

#[overseer::contextbounds(ApprovalDistribution, prefix = self::overseer)]
impl ApprovalDistribution {
	/// Create a new instance of the [`ApprovalDistribution`] subsystem.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	async fn run<Context>(self, ctx: Context) {
		let mut state = State::default();

		// According to the docs of `rand`, this is a ChaCha12 RNG in practice
		// and will always be chosen for strong performance and security properties.
		let mut rng = rand::rngs::StdRng::from_entropy();
		self.run_inner(ctx, &mut state, &mut rng).await
	}

	/// Used for testing.
	async fn run_inner<Context>(
		self,
		mut ctx: Context,
		state: &mut State,
		rng: &mut (impl CryptoRng + Rng),
	) {
		loop {
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					gum::debug!(target: LOG_TARGET, err = ?e, "Failed to receive a message from Overseer, exiting");
					return
				},
			};
			match message {
				FromOrchestra::Communication { msg } =>
					Self::handle_incoming(&mut ctx, state, msg, &self.metrics, rng).await,
				FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					..
				})) => {
					gum::trace!(target: LOG_TARGET, "active leaves signal (ignored)");
					// the relay chain blocks relevant to the approval subsystems
					// are those that are available, but not finalized yet
					// actived and deactivated heads hence are irrelevant to this subsystem
				},
				FromOrchestra::Signal(OverseerSignal::BlockFinalized(_hash, number)) => {
					gum::trace!(target: LOG_TARGET, number = %number, "finalized signal");
					state.handle_block_finalized(&mut ctx, &self.metrics, number).await;
				},
				FromOrchestra::Signal(OverseerSignal::Conclude) => return,
			}
		}
	}

	async fn handle_incoming<Context>(
		ctx: &mut Context,
		state: &mut State,
		msg: ApprovalDistributionMessage,
		metrics: &Metrics,
		rng: &mut (impl CryptoRng + Rng),
	) {
		match msg {
			ApprovalDistributionMessage::NetworkBridgeUpdate(event) => {
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
			ApprovalDistributionMessage::GetApprovalSignatures(indices, tx) => {
				let sigs = state.get_approval_signatures(indices);
				if let Err(_) = tx.send(sigs) {
					gum::debug!(
						target: LOG_TARGET,
						"Sending back approval signatures failed, oneshot got closed"
					);
				}
			},
		}
	}
}

#[overseer::subsystem(ApprovalDistribution, error=SubsystemError, prefix=self::overseer)]
impl<Context> ApprovalDistribution {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self.run(ctx).map(|_| Ok(())).boxed();

		SpawnedSubsystem { name: "approval-distribution-subsystem", future }
	}
}

/// Ensures the batch size is always at least 1 element.
const fn ensure_size_not_zero(size: usize) -> usize {
	if 0 == size {
		panic!("Batch size must be at least 1 (MAX_NOTIFICATION_SIZE constant is too low)",);
	}

	size
}

/// The maximum amount of assignments per batch is 33% of maximum allowed by protocol.
/// This is an arbitrary value. Bumping this up increases the maximum amount of approvals or assignments
/// we send in a single message to peers. Exceeding `MAX_NOTIFICATION_SIZE` will violate the protocol
/// configuration.
pub const MAX_ASSIGNMENT_BATCH_SIZE: usize = ensure_size_not_zero(
	MAX_NOTIFICATION_SIZE as usize /
		std::mem::size_of::<(IndirectAssignmentCert, CandidateIndex)>() /
		3,
);

/// The maximum amount of approvals per batch is 33% of maximum allowed by protocol.
pub const MAX_APPROVAL_BATCH_SIZE: usize = ensure_size_not_zero(
	MAX_NOTIFICATION_SIZE as usize / std::mem::size_of::<IndirectSignedApprovalVote>() / 3,
);

/// Send assignments while honoring the `max_notification_size` of the protocol.
///
/// Splitting the messages into multiple notifications allows more granular processing at the
/// destination, such that the subsystem doesn't get stuck for long processing a batch
/// of assignments and can `select!` other tasks.
pub(crate) async fn send_assignments_batched(
	sender: &mut impl overseer::ApprovalDistributionSenderTrait,
	assignments: Vec<(IndirectAssignmentCert, CandidateIndex)>,
	peer: PeerId,
) {
	let mut batches = assignments.into_iter().peekable();

	while batches.peek().is_some() {
		let batch: Vec<_> = batches.by_ref().take(MAX_ASSIGNMENT_BATCH_SIZE).collect();

		sender
			.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				vec![peer],
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(batch),
				)),
			))
			.await;
	}
}

/// Send approvals while honoring the `max_notification_size` of the protocol.
pub(crate) async fn send_approvals_batched(
	sender: &mut impl overseer::ApprovalDistributionSenderTrait,
	approvals: Vec<IndirectSignedApprovalVote>,
	peer: PeerId,
) {
	let mut batches = approvals.into_iter().peekable();

	while batches.peek().is_some() {
		let batch: Vec<_> = batches.by_ref().take(MAX_APPROVAL_BATCH_SIZE).collect();

		sender
			.send_message(NetworkBridgeTxMessage::SendValidationMessage(
				vec![peer],
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(batch),
				)),
			))
			.await;
	}
}
