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

#[cfg(test)]
mod tests;


use std::collections::{BTreeMap, HashMap, HashSet, hash_map};
use futures::{channel::oneshot, FutureExt as _};
use polkadot_primitives::v1::{
	Hash, BlockNumber, ValidatorIndex, ValidatorSignature, CandidateIndex,
};
use polkadot_node_primitives::{
	approval::{AssignmentCert, BlockApprovalMeta, IndirectSignedApprovalVote, IndirectAssignmentCert},
};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, ApprovalDistributionMessage, ApprovalVotingMessage, NetworkBridgeMessage,
		AssignmentCheckResult, ApprovalCheckResult, NetworkBridgeEvent,
	},
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_node_network_protocol::{
	PeerId, View, v1 as protocol_v1, UnifiedReputationChange as Rep,

};

const LOG_TARGET: &str = "approval_distribution";

const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("Peer sent an out-of-view assignment or approval");
const COST_DUPLICATE_MESSAGE: Rep = Rep::CostMinorRepeated("Peer sent identical messages");
const COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE: Rep = Rep::CostMinor("The vote was valid but too far in the future");
const COST_INVALID_MESSAGE: Rep = Rep::CostMajor("The vote was bad");

const BENEFIT_VALID_MESSAGE: Rep = Rep::BenefitMinor("Peer sent a valid message");
const BENEFIT_VALID_MESSAGE_FIRST: Rep = Rep::BenefitMinorFirst("Valid message with new information");

/// The Approval Distribution subsystem.
pub struct ApprovalDistribution {
	metrics: Metrics,
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

	/// Peer view data is partially stored here, and partially inline within the [`BlockEntry`]s
	peer_views: HashMap<PeerId, View>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum MessageFingerprint {
	Assignment(Hash, CandidateIndex, ValidatorIndex),
	Approval(Hash, CandidateIndex, ValidatorIndex),
}

#[derive(Debug, Clone, Default)]
struct Knowledge {
	known_messages: HashSet<MessageFingerprint>,
}

/// Information about blocks in our current view as well as whether peers know of them.
struct BlockEntry {
	/// Peers who we know are aware of this block and thus, the candidates within it.
	/// This maps to their knowledge of messages.
	known_by: HashMap<PeerId, Knowledge>,
	/// The number of the block.
	number: BlockNumber,
	/// The parent hash of the block.
	parent_hash: Hash,
	/// Our knowledge of messages.
	knowledge: Knowledge,
	/// A votes entry for each candidate indexed by [`CandidateIndex`].
	candidates: Vec<CandidateEntry>,
}

#[derive(Debug)]
enum ApprovalState {
	Assigned(AssignmentCert),
	Approved(AssignmentCert, ValidatorSignature),
}

/// Information about candidates in the context of a particular block they are included in.
/// In other words, multiple `CandidateEntry`s may exist for the same candidate,
/// if it is included by multiple blocks - this is likely the case when there are forks.
#[derive(Debug, Default)]
struct CandidateEntry {
	approvals: HashMap<ValidatorIndex, ApprovalState>,
}

#[derive(Debug, Clone)]
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
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
		metrics: &Metrics,
		event: NetworkBridgeEvent<protocol_v1::ApprovalDistributionMessage>,
	) {
		match event {
			NetworkBridgeEvent::PeerConnected(peer_id, _role) => {
				// insert a blank view if none already present
				self.peer_views.entry(peer_id).or_default();
			}
			NetworkBridgeEvent::PeerDisconnected(peer_id) => {
				self.peer_views.remove(&peer_id);
				self.blocks.iter_mut().for_each(|(_hash, entry)| {
					entry.known_by.remove(&peer_id);
				})
			}
			NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
				self.handle_peer_view_change(ctx, peer_id, view).await;
			}
			NetworkBridgeEvent::OurViewChange(view) => {
				for head in view.iter() {
					if !self.blocks.contains_key(head) {
						self.pending_known.entry(*head).or_default();
					}
				}

				self.pending_known.retain(|h, _| view.contains(h));
			}
			NetworkBridgeEvent::PeerMessage(peer_id, msg) => {
				self.process_incoming_peer_message(ctx, metrics, peer_id, msg).await;
			}
		}
	}

	async fn handle_new_blocks(
		&mut self,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
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
					});
					new_hashes.insert(meta.hash.clone());
				}
				_ => continue,
			}
			self.blocks_by_number.entry(meta.number).or_default().push(meta.hash);
		}

		tracing::debug!(
			target: LOG_TARGET,
			"Got new blocks {:?}",
			metas.iter().map(|m| (m.hash, m.number)).collect::<Vec<_>>(),
		);

		{
			let pending_now_known = self.pending_known.keys()
				.filter(|k| self.blocks.contains_key(k))
				.copied()
				.collect::<Vec<_>>();

			let to_import = pending_now_known.into_iter()
				.filter_map(|k| self.pending_known.remove(&k))
				.flatten()
				.collect::<Vec<_>>();

			for (peer_id, message) in to_import {
				match message {
					PendingMessage::Assignment(assignment, claimed_index) => {
						self.import_and_circulate_assignment(
							ctx,
							metrics,
							MessageSource::Peer(peer_id),
							assignment,
							claimed_index,
						).await;
					}
					PendingMessage::Approval(approval_vote) => {
						self.import_and_circulate_approval(
							ctx,
							metrics,
							MessageSource::Peer(peer_id),
							approval_vote,
						).await;
					}
				}
			}
		}

		for (peer_id, view) in self.peer_views.iter() {
			let intersection = view.iter().filter(|h| new_hashes.contains(h));
			let view_intersection = View::new(
				intersection.cloned(),
				view.finalized_number,
			);
			Self::unify_with_peer(
				&mut self.blocks,
				ctx,
				peer_id.clone(),
				view_intersection,
			).await;
		}
	}

	async fn process_incoming_peer_message(
		&mut self,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
		metrics: &Metrics,
		peer_id: PeerId,
		msg: protocol_v1::ApprovalDistributionMessage,
	) {
		match msg {
			protocol_v1::ApprovalDistributionMessage::Assignments(assignments) => {
				tracing::trace!(
					target: LOG_TARGET,
					peer_id = %peer_id,
					num = assignments.len(),
					"Processing assignments from a peer",
				);
				for (assignment, claimed_index) in assignments.into_iter() {
					if let Some(pending) = self.pending_known.get_mut(&assignment.block_hash) {
						pending.push((
							peer_id.clone(),
							PendingMessage::Assignment(assignment, claimed_index),
						));

						continue;
					}

					self.import_and_circulate_assignment(
						ctx,
						metrics,
						MessageSource::Peer(peer_id.clone()),
						assignment,
						claimed_index,
					).await;
				}
			}
			protocol_v1::ApprovalDistributionMessage::Approvals(approvals) => {
				tracing::trace!(
					target: LOG_TARGET,
					peer_id = %peer_id,
					num = approvals.len(),
					"Processing approvals from a peer",
				);
				for approval_vote in approvals.into_iter() {
					if let Some(pending) = self.pending_known.get_mut(&approval_vote.block_hash) {
						pending.push((
							peer_id.clone(),
							PendingMessage::Approval(approval_vote),
						));

						continue;
					}

					self.import_and_circulate_approval(
						ctx,
						metrics,
						MessageSource::Peer(peer_id.clone()),
						approval_vote,
					).await;
				}
			}
		}
	}

	async fn handle_peer_view_change(
		&mut self,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
		peer_id: PeerId,
		view: View,
	) {
		Self::unify_with_peer(&mut self.blocks, ctx, peer_id.clone(), view.clone()).await;
		let finalized_number = view.finalized_number;
		let old_view = self.peer_views.insert(peer_id.clone(), view);
		let old_finalized_number = old_view.map(|v| v.finalized_number).unwrap_or(0);

		// we want to prune every block known_by peer up to (including) view.finalized_number
		let blocks = &mut self.blocks;
		// the `BTreeMap::range` is constrained by stored keys
		// so the loop won't take ages if the new finalized_number skyrockets
		// but we need to make sure the range is not empty, otherwise it will panic
		// it shouldn't be, we make sure of this in the network bridge
		let range = old_finalized_number..=finalized_number;
		if !range.is_empty() {
			self.blocks_by_number
			.range(range)
			.map(|(_number, hashes)| hashes)
			.flatten()
			.for_each(|hash| {
				if let Some(entry) = blocks.get_mut(hash) {
					entry.known_by.remove(&peer_id);
				}
			});
		}
	}

	fn handle_block_finalized(
		&mut self,
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
		old_blocks.values()
			.flatten()
			.for_each(|h| {
				self.blocks.remove(h);
			});
	}

	async fn import_and_circulate_assignment(
		&mut self,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
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
					modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
				}
				return;
			}
		};

		// compute a fingerprint of the assignment
		let fingerprint = MessageFingerprint::Assignment(
			block_hash,
			claimed_candidate_index,
			validator_index,
		);

		if let Some(peer_id) = source.peer_id() {
			// check if our knowledge of the peer already contains this assignment
			match entry.known_by.entry(peer_id.clone()) {
				hash_map::Entry::Occupied(knowledge) => {
					if knowledge.get().known_messages.contains(&fingerprint) {
						modify_reputation(ctx, peer_id, COST_DUPLICATE_MESSAGE).await;
						return;
					}
				}
				hash_map::Entry::Vacant(_) => {
					modify_reputation(ctx, peer_id.clone(), COST_UNEXPECTED_MESSAGE).await;
				}
			}

			// if the assignment is known to be valid, reward the peer
			if entry.knowledge.known_messages.contains(&fingerprint) {
				modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE).await;
				if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
					peer_knowledge.known_messages.insert(fingerprint.clone());
				}
				return;
			}

			let (tx, rx) = oneshot::channel();

			ctx.send_message(AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment.clone(),
				claimed_candidate_index,
				tx,
			))).await;

			let result = match rx.await {
				Ok(result) => result,
				Err(_) => {
					tracing::debug!(
						target: LOG_TARGET,
						"The approval voting subsystem is down",
					);
					return;
				}
			};

			match result {
				AssignmentCheckResult::Accepted => {
					modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;
					entry.knowledge.known_messages.insert(fingerprint.clone());
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.known_messages.insert(fingerprint.clone());
					}
				}
				AssignmentCheckResult::AcceptedDuplicate => {
					// "duplicate" assignments aren't necessarily equal.
					// There is more than one way each validator can be assigned to each core.
					// cf. https://github.com/paritytech/polkadot/pull/2160#discussion_r557628699
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.known_messages.insert(fingerprint);
					}
					return;
				}
				AssignmentCheckResult::TooFarInFuture => {
					modify_reputation(ctx, peer_id, COST_ASSIGNMENT_TOO_FAR_IN_THE_FUTURE).await;
					return;
				}
				AssignmentCheckResult::Bad => {
					modify_reputation(ctx, peer_id, COST_INVALID_MESSAGE).await;
					tracing::info!(
						target: LOG_TARGET,
						peer = ?peer_id,
						"Got a bad assignment from peer",
					);
					return;
				}
			}
		} else {
			if !entry.knowledge.known_messages.insert(fingerprint.clone()) {
				// if we already imported an assignment, there is no need to distribute it again
				return;
			}
		}

		// Invariant: none of the peers except for the `source` know about the assignment.
		metrics.on_assignment_imported();

		match entry.candidates.get_mut(claimed_candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Assigned
				// unless the approval state is set already
				candidate_entry.approvals
					.entry(validator_index)
					.or_insert_with(|| ApprovalState::Assigned(assignment.cert.clone()));
			}
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					hash = ?block_hash,
					?claimed_candidate_index,
					"Expected a candidate entry on import_and_circulate_assignment",
				);
			}
		}

		// Dispatch a ApprovalDistributionV1Message::Assignment(assignment, candidate_index)
		// to all peers in the BlockEntry's known_by set who know about the block,
		// excluding the peer in the source, if source has kind MessageSource::Peer.
		let maybe_peer_id = source.peer_id();
		let peers = entry
			.known_by
			.keys()
			.cloned()
			.filter(|key| maybe_peer_id.as_ref().map_or(true, |id| id != key))
			.collect::<Vec<_>>();

		let assignments = vec![(assignment, claimed_candidate_index)];

		// Add the fingerprint of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(entry) = entry.known_by.get_mut(peer) {
				entry.known_messages.insert(fingerprint.clone());
			}
		}

		if !peers.is_empty() {
			tracing::trace!(
				target: LOG_TARGET,
				"Sending assignment (block={}, index={})to {} peers",
				block_hash,
				claimed_candidate_index,
				peers.len(),
			);

			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				),
			).into()).await;
		}
	}

	async fn import_and_circulate_approval(
		&mut self,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
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
					modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
				}
				return;
			}
		};

		// compute a fingerprint of the approval
		let fingerprint = MessageFingerprint::Approval(
			block_hash.clone(),
			candidate_index,
			validator_index,
		);

		if let Some(peer_id) = source.peer_id() {
			let assignment_fingerprint = MessageFingerprint::Assignment(
				block_hash.clone(),
				candidate_index,
				validator_index,
			);

			if !entry.knowledge.known_messages.contains(&assignment_fingerprint) {
				modify_reputation(ctx, peer_id, COST_UNEXPECTED_MESSAGE).await;
				return;
			}

			// check if our knowledge of the peer already contains this approval
			match entry.known_by.entry(peer_id.clone()) {
				hash_map::Entry::Occupied(knowledge) => {
					if knowledge.get().known_messages.contains(&fingerprint) {
						modify_reputation(ctx, peer_id, COST_DUPLICATE_MESSAGE).await;
						return;
					}
				}
				hash_map::Entry::Vacant(_) => {
					modify_reputation(ctx, peer_id.clone(), COST_UNEXPECTED_MESSAGE).await;
				}
			}

			// if the approval is known to be valid, reward the peer
			if entry.knowledge.known_messages.contains(&fingerprint) {
				modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE).await;
				if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
					peer_knowledge.known_messages.insert(fingerprint.clone());
				}
				return;
			}

			let (tx, rx) = oneshot::channel();

			ctx.send_message(AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportApproval(
				vote.clone(),
				tx,
			))).await;

			let result = match rx.await {
				Ok(result) => result,
				Err(_) => {
					tracing::debug!(
						target: LOG_TARGET,
						"The approval voting subsystem is down",
					);
					return;
				}
			};

			match result {
				ApprovalCheckResult::Accepted => {
					modify_reputation(ctx, peer_id.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;

					entry.knowledge.known_messages.insert(fingerprint.clone());
					if let Some(peer_knowledge) = entry.known_by.get_mut(&peer_id) {
						peer_knowledge.known_messages.insert(fingerprint.clone());
					}
				}
				ApprovalCheckResult::Bad => {
					modify_reputation(ctx, peer_id, COST_INVALID_MESSAGE).await;
					tracing::info!(
						target: LOG_TARGET,
						peer = ?peer_id,
						"Got a bad approval from peer",
					);
					return;
				}
			}
		} else {
			if !entry.knowledge.known_messages.insert(fingerprint.clone()) {
				// if we already imported an approval, there is no need to distribute it again
				return;
			}
		}

		// Invariant: none of the peers except for the `source` know about the approval.
		metrics.on_approval_imported();

		match entry.candidates.get_mut(candidate_index as usize) {
			Some(candidate_entry) => {
				// set the approval state for validator_index to Approved
				// it should be in assigned state already
				match candidate_entry.approvals.remove(&validator_index) {
					Some(ApprovalState::Assigned(cert)) => {
						candidate_entry.approvals.insert(
							validator_index,
							ApprovalState::Approved(cert, vote.signature.clone()),
						);
					}
					_ => {
						tracing::warn!(
							target: LOG_TARGET,
							hash = ?block_hash,
							?candidate_index,
							"Expected a candidate entry with `ApprovalState::Assigned`",
						);
					}
				}
			}
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					hash = ?block_hash,
					?candidate_index,
					"Expected a candidate entry on import_and_circulate_approval",
				);
			}
		}

		// Dispatch a ApprovalDistributionV1Message::Approval(vote)
		// to all peers in the BlockEntry's known_by set who know about the block,
		// excluding the peer in the source, if source has kind MessageSource::Peer.
		let maybe_peer_id = source.peer_id();
		let peers = entry
			.known_by
			.keys()
			.cloned()
			.filter(|key| maybe_peer_id.as_ref().map_or(true, |id| id != key))
			.collect::<Vec<_>>();

		// Add the fingerprint of the assignment to the knowledge of each peer.
		for peer in peers.iter() {
			// we already filtered peers above, so this should always be Some
			if let Some(entry) = entry.known_by.get_mut(peer) {
				entry.known_messages.insert(fingerprint.clone());
			}
		}

		let approvals = vec![vote];
		if !peers.is_empty() {
			tracing::trace!(
				target: LOG_TARGET,
				"Sending approval (block={}, index={})to {} peers",
				block_hash,
				candidate_index,
				peers.len(),
			);

			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals)
				),
			).into()).await;
		}
	}

	async fn unify_with_peer(
		entries: &mut HashMap<Hash, BlockEntry>,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
		peer_id: PeerId,
		view: View,
	) {
		let mut to_send = HashSet::new();

		let view_finalized_number = view.finalized_number;
		for head in view.into_iter() {
			let mut block = head;
			let interesting_blocks = std::iter::from_fn(|| {
				// step 2.
				let entry = match entries.get_mut(&block) {
					Some(entry) if entry.number > view_finalized_number => entry,
					_ => return None,
				};
				let interesting_block = match entry.known_by.entry(peer_id.clone()) {
					// step 3.
					hash_map::Entry::Occupied(_) => return None,
					// step 4.
					hash_map::Entry::Vacant(vacant) => {
						vacant.insert(entry.knowledge.clone());
						block
					}
				};
				// step 5.
				block = entry.parent_hash.clone();
				Some(interesting_block)
			});
			to_send.extend(interesting_blocks);
		}
		// step 6.
		// send all assignments and approvals for all candidates in those blocks to the peer
		Self::send_gossip_messages_to_peer(
			entries,
			ctx,
			peer_id,
			to_send
		).await;
	}

	async fn send_gossip_messages_to_peer(
		entries: &HashMap<Hash, BlockEntry>,
		ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
		peer_id: PeerId,
		blocks: HashSet<Hash>,
	) {
		let mut assignments = Vec::new();
		let mut approvals = Vec::new();

		for block in blocks.into_iter() {
			let entry = match entries.get(&block) {
				Some(entry) => entry,
				None => continue, // should be unreachable
			};

			tracing::trace!(
				target: LOG_TARGET,
				"Sending all assignments and approvals in block {} to peer {}",
				block,
				peer_id,
			);

			for (candidate_index, candidate_entry) in entry.candidates.iter().enumerate() {
				let candidate_index = candidate_index as u32;
				for (validator_index, approval_state) in candidate_entry.approvals.iter() {
					match approval_state {
						ApprovalState::Assigned(cert) => {
							assignments.push((IndirectAssignmentCert {
								block_hash: block.clone(),
								validator: validator_index.clone(),
								cert: cert.clone(),
							}, candidate_index.clone()));
						}
						ApprovalState::Approved(_, signature) => {
							approvals.push(IndirectSignedApprovalVote {
								block_hash: block.clone(),
								validator: validator_index.clone(),
								candidate_index: candidate_index.clone(),
								signature: signature.clone(),
							});
						}
					}
				}
			}
		}

		if !assignments.is_empty() {
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id.clone()],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				),
			).into()).await;
		}

		if !approvals.is_empty() {
			ctx.send_message(NetworkBridgeMessage::SendValidationMessage(
				vec![peer_id],
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals)
				),
			).into()).await;
		}
	}
}


/// Modify the reputation of a peer based on its behavior.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn modify_reputation(
	ctx: &mut impl SubsystemContext<Message = ApprovalDistributionMessage>,
	peer_id: PeerId,
	rep: Rep,
) {
	tracing::trace!(
		target: LOG_TARGET,
		reputation = ?rep,
		?peer_id,
		"Reputation change for peer",
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer_id, rep),
	)).await;
}

impl ApprovalDistribution {
	/// Create a new instance of the [`ApprovalDistribution`] subsystem.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run<Context>(self, ctx: Context)
	where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		let mut state = State::default();
		self.run_inner(ctx, &mut state).await
	}

	/// Used for testing.
	#[tracing::instrument(skip(self, ctx, state), fields(subsystem = LOG_TARGET))]
	async fn run_inner<Context>(self, mut ctx: Context, state: &mut State)
	where
		Context: SubsystemContext<Message = ApprovalDistributionMessage>,
	{
		loop {
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					tracing::debug!(target: LOG_TARGET, err = ?e, "Failed to receive a message from Overseer, exiting");
					return;
				},
			};
			match message {
				FromOverseer::Communication {
					msg: ApprovalDistributionMessage::NetworkBridgeUpdateV1(event),
				} => {
					tracing::debug!(target: LOG_TARGET, "Processing network message");
					state.handle_network_msg(&mut ctx, &self.metrics, event).await;
				}
				FromOverseer::Communication {
					msg: ApprovalDistributionMessage::NewBlocks(metas),
				} => {
					tracing::debug!(target: LOG_TARGET, "Processing NewBlocks");
					state.handle_new_blocks(&mut ctx, &self.metrics, metas).await;
				}
				FromOverseer::Communication {
					msg: ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index),
				} => {
					tracing::debug!(
						target: LOG_TARGET,
						"Distributing our assignment on candidate (block={}, index={})",
						cert.block_hash,
						candidate_index,
					);

					state.import_and_circulate_assignment(
						&mut ctx,
						&self.metrics,
						MessageSource::Local,
						cert,
						candidate_index,
					).await;
				}
				FromOverseer::Communication {
					msg: ApprovalDistributionMessage::DistributeApproval(vote),
				} => {
					tracing::debug!(
						target: LOG_TARGET,
						"Distributing our approval vote on candidate (block={}, index={})",
						vote.block_hash,
						vote.candidate_index,
					);

					state.import_and_circulate_approval(
						&mut ctx,
						&self.metrics,
						MessageSource::Local,
						vote,
					).await;
				}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { .. })) => {
					tracing::trace!(target: LOG_TARGET, "active leaves signal (ignored)");
					// handled by NewBlocks
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number)) => {
					tracing::trace!(target: LOG_TARGET, number = %number, "finalized signal");
					state.handle_block_finalized(number);
				},
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return;
				}
			}
		}
	}
}

impl<C> Subsystem<C> for ApprovalDistribution
where
	C: SubsystemContext<Message = ApprovalDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = self.run(ctx)
			.map(|_| Ok(()))
			.boxed();

		SpawnedSubsystem {
			name: "approval-distribution-subsystem",
			future,
		}
	}
}


/// Approval Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	assignments_imported_total: prometheus::Counter<prometheus::U64>,
	approvals_imported_total: prometheus::Counter<prometheus::U64>,
}

impl Metrics {
	fn on_assignment_imported(&self) {
		if let Some(metrics) = &self.0 {
			metrics.assignments_imported_total.inc();
		}
	}

	fn on_approval_imported(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_imported_total.inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			assignments_imported_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_assignments_imported_total",
					"Number of valid assignments imported locally or from other peers.",
				)?,
				registry,
			)?,
			approvals_imported_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_approvals_imported_total",
					"Number of valid approvals imported locally or from other peers.",
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
