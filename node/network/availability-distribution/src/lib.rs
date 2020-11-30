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

//! The availability distribution
//!
//! Transforms `AvailableData` into erasure chunks, which are distributed to peers
//! who are interested in the relevant candidates.
//! Gossip messages received from other peers are verified and gossiped to interested
//! peers. Verified in this context means, the erasure chunks contained merkle proof
//! is checked.

#![deny(unused_crate_dependencies, unused_qualifications)]

use parity_scale_codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt, TryFutureExt};

use sp_core::crypto::Public;
use sp_keystore::{CryptoStore, SyncCryptoStorePtr};

use polkadot_erasure_coding::branch_hash;
use polkadot_node_network_protocol::{
	v1 as protocol_v1, NetworkBridgeEvent, PeerId, ReputationChange as Rep, View,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_primitives::v1::{
	BlakeTwo256, CommittedCandidateReceipt, CoreState, ErasureChunk, Hash, HashT, Id as ParaId,
	SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID, CandidateHash,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
	NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest,
};
use polkadot_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem,
	SubsystemContext, SubsystemError,
};
use std::collections::{HashMap, HashSet};
use std::iter;
use thiserror::Error;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "availability_distribution";

#[derive(Debug, Error)]
enum Error {
	#[error("Response channel to obtain PendingAvailability failed")]
	QueryPendingAvailabilityResponseChannel(#[source] oneshot::Canceled),
	#[error("RuntimeAPI to obtain PendingAvailability failed")]
	QueryPendingAvailability(#[source] RuntimeApiError),

	#[error("Response channel to obtain StoreChunk failed")]
	StoreChunkResponseChannel(#[source] oneshot::Canceled),

	#[error("Response channel to obtain QueryChunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

	#[error("Response channel to obtain QueryAncestors failed")]
	QueryAncestorsResponseChannel(#[source] oneshot::Canceled),
	#[error("RuntimeAPI to obtain QueryAncestors failed")]
	QueryAncestors(#[source] ChainApiError),

	#[error("Response channel to obtain QuerySession failed")]
	QuerySessionResponseChannel(#[source] oneshot::Canceled),
	#[error("RuntimeAPI to obtain QuerySession failed")]
	QuerySession(#[source] RuntimeApiError),

	#[error("Response channel to obtain QueryValidators failed")]
	QueryValidatorsResponseChannel(#[source] oneshot::Canceled),
	#[error("RuntimeAPI to obtain QueryValidators failed")]
	QueryValidators(#[source] RuntimeApiError),

	#[error("Response channel to obtain AvailabilityCores failed")]
	AvailabilityCoresResponseChannel(#[source] oneshot::Canceled),
	#[error("RuntimeAPI to obtain AvailabilityCores failed")]
	AvailabilityCores(#[source] RuntimeApiError),

	#[error("Response channel to obtain AvailabilityCores failed")]
	QueryAvailabilityResponseChannel(#[source] oneshot::Canceled),

	#[error("Receive channel closed")]
	IncomingMessageChannel(#[source] SubsystemError),
}

type Result<T> = std::result::Result<T, Error>;

const COST_MERKLE_PROOF_INVALID: Rep = Rep::new(-100, "Merkle proof was invalid");
const COST_NOT_A_LIVE_CANDIDATE: Rep = Rep::new(-51, "Candidate is not live");
const COST_PEER_DUPLICATE_MESSAGE: Rep = Rep::new(-500, "Peer sent identical messages");
const BENEFIT_VALID_MESSAGE_FIRST: Rep = Rep::new(15, "Valid message with new information");
const BENEFIT_VALID_MESSAGE: Rep = Rep::new(10, "Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AvailabilityGossipMessage {
	/// Anchor hash of the candidate the `ErasureChunk` is associated to.
	pub candidate_hash: CandidateHash,
	/// The erasure chunk, a encoded information part of `AvailabilityData`.
	pub erasure_chunk: ErasureChunk,
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone, Debug)]
struct ProtocolState {
	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our own view.
	view: View,

	/// Caches a mapping of relay parents or ancestor to live candidate receipts.
	/// Allows fast intersection of live candidates with views and consecutive unioning.
	/// Maps relay parent / ancestor -> live candidate receipts + its hash.
	receipts: HashMap<Hash, HashSet<(CandidateHash, CommittedCandidateReceipt)>>,

	/// Allow reverse caching of view checks.
	/// Maps candidate hash -> relay parent for extracting meta information from `PerRelayParent`.
	/// Note that the presence of this is not sufficient to determine if deletion is OK, i.e.
	/// two histories could cover this.
	reverse: HashMap<CandidateHash, Hash>,

	/// Keeps track of which candidate receipts are required due to ancestors of which relay parents
	/// of our view.
	/// Maps ancestor -> relay parents in view
	ancestry: HashMap<Hash, HashSet<Hash>>,

	/// Track things needed to start and stop work on a particular relay parent.
	per_relay_parent: HashMap<Hash, PerRelayParent>,

	/// Track data that is specific to a candidate.
	per_candidate: HashMap<CandidateHash, PerCandidate>,
}

#[derive(Debug, Clone, Default)]
struct PerCandidate {
	/// A Candidate and a set of known erasure chunks in form of messages to be gossiped / distributed if the peer view wants that.
	/// This is _across_ peers and not specific to a particular one.
	/// candidate hash + erasure chunk index -> gossip message
	message_vault: HashMap<u32, AvailabilityGossipMessage>,

	/// Track received candidate hashes and validator indices from peers.
	received_messages: HashMap<PeerId, HashSet<(CandidateHash, ValidatorIndex)>>,

	/// Track already sent candidate hashes and the erasure chunk index to the peers.
	sent_messages: HashMap<PeerId, HashSet<(CandidateHash, ValidatorIndex)>>,

	/// The set of validators.
	validators: Vec<ValidatorId>,

	/// If this node is a validator, note the index in the validator set.
	validator_index: Option<ValidatorIndex>,
}

impl PerCandidate {
	/// Returns `true` iff the given `message` is required by the given `peer`.
	fn message_required_by_peer(&self, peer: &PeerId, message: &(CandidateHash, ValidatorIndex)) -> bool {
		self.received_messages.get(peer).map(|v| !v.contains(message)).unwrap_or(true)
			&& self.sent_messages.get(peer).map(|v| !v.contains(message)).unwrap_or(true)
	}
}

#[derive(Debug, Clone, Default)]
struct PerRelayParent {
	/// Set of `K` ancestors for this relay parent.
	ancestors: Vec<Hash>,
}

impl ProtocolState {
	/// Collects the relay_parents ancestors including the relay parents themselfes.
	#[tracing::instrument(level = "trace", skip(relay_parents), fields(subsystem = LOG_TARGET))]
	fn extend_with_ancestors<'a>(
		&'a self,
		relay_parents: impl IntoIterator<Item = &'a Hash> + 'a,
	) -> HashSet<Hash> {
		relay_parents
			.into_iter()
			.map(|relay_parent| {
				self.per_relay_parent
					.get(relay_parent)
					.into_iter()
					.map(|per_relay_parent| per_relay_parent.ancestors.iter().cloned())
					.flatten()
					.chain(iter::once(*relay_parent))
			})
			.flatten()
			.collect::<HashSet<Hash>>()
	}

	/// Unionize all cached entries for the given relay parents and its ancestors.
	/// Ignores all non existent relay parents, so this can be used directly with a peers view.
	/// Returns a map from candidate hash -> receipt
	#[tracing::instrument(level = "trace", skip(relay_parents), fields(subsystem = LOG_TARGET))]
	fn cached_live_candidates_unioned<'a>(
		&'a self,
		relay_parents: impl IntoIterator<Item = &'a Hash> + 'a,
	) -> HashMap<CandidateHash, CommittedCandidateReceipt> {
		let relay_parents_and_ancestors = self.extend_with_ancestors(relay_parents);
		relay_parents_and_ancestors
			.into_iter()
			.filter_map(|relay_parent_or_ancestor| self.receipts.get(&relay_parent_or_ancestor))
			.map(|receipt_set| receipt_set.into_iter())
			.flatten()
			.map(|(receipt_hash, receipt)| (receipt_hash.clone(), receipt.clone()))
			.collect()
	}

	#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
	async fn add_relay_parent<Context>(
		&mut self,
		ctx: &mut Context,
		relay_parent: Hash,
		validators: Vec<ValidatorId>,
		validator_index: Option<ValidatorIndex>,
	) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		let candidates = query_live_candidates(ctx, self, std::iter::once(relay_parent)).await?;

		// register the relation of relay_parent to candidate..
		// ..and the reverse association.
		for (relay_parent_or_ancestor, (receipt_hash, receipt)) in candidates.clone() {
			self.reverse
				.insert(receipt_hash.clone(), relay_parent_or_ancestor.clone());
			let per_candidate = self.per_candidate.entry(receipt_hash.clone()).or_default();
			per_candidate.validator_index = validator_index.clone();
			per_candidate.validators = validators.clone();

			self.receipts
				.entry(relay_parent_or_ancestor)
				.or_default()
				.insert((receipt_hash, receipt));
		}

		// collect the ancestors again from the hash map
		let ancestors = candidates
			.iter()
			.filter_map(|(ancestor_or_relay_parent, _receipt)| {
				if ancestor_or_relay_parent == &relay_parent {
					None
				} else {
					Some(*ancestor_or_relay_parent)
				}
			})
			.collect::<Vec<Hash>>();

		// mark all the ancestors as "needed" by this newly added relay parent
		for ancestor in ancestors.iter() {
			self.ancestry
				.entry(ancestor.clone())
				.or_default()
				.insert(relay_parent);
		}

		self.per_relay_parent
			.entry(relay_parent)
			.or_default()
			.ancestors = ancestors;

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn remove_relay_parent(&mut self, relay_parent: &Hash) {
		// we might be ancestor of some other relay_parent
		if let Some(ref mut descendants) = self.ancestry.get_mut(relay_parent) {
			// if we were the last user, and it is
			// not explicitly set to be worked on by the overseer
			if descendants.is_empty() {
				// remove from the ancestry index
				self.ancestry.remove(relay_parent);
				// and also remove the actual receipt
				if let Some(candidates) = self.receipts.remove(relay_parent) {
					candidates.into_iter().for_each(|c| { self.per_candidate.remove(&c.0); });
				}
			}
		}
		if let Some(per_relay_parent) = self.per_relay_parent.remove(relay_parent) {
			// remove all "references" from the hash maps and sets for all ancestors
			for ancestor in per_relay_parent.ancestors {
				// one of our decendants might be ancestor of some other relay_parent
				if let Some(ref mut descendants) = self.ancestry.get_mut(&ancestor) {
					// we do not need this descendant anymore
					descendants.remove(&relay_parent);
					// if we were the last user, and it is
					// not explicitly set to be worked on by the overseer
					if descendants.is_empty() && !self.per_relay_parent.contains_key(&ancestor) {
						// remove from the ancestry index
						self.ancestry.remove(&ancestor);
						// and also remove the actual receipt
						if let Some(candidates) = self.receipts.remove(&ancestor) {
							candidates.into_iter().for_each(|c| { self.per_candidate.remove(&c.0); });
						}
					}
				}
			}
		}
	}
}

/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
#[tracing::instrument(level = "trace", skip(ctx, keystore, metrics), fields(subsystem = LOG_TARGET))]
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	keystore: &SyncCryptoStorePtr,
	state: &mut ProtocolState,
	metrics: &Metrics,
	bridge_message: NetworkBridgeEvent<protocol_v1::AvailabilityDistributionMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	match bridge_message {
		NetworkBridgeEvent::PeerConnected(peerid, _role) => {
			// insert if none already present
			state.peer_views.entry(peerid).or_default();
		}
		NetworkBridgeEvent::PeerDisconnected(peerid) => {
			// get rid of superfluous data
			state.peer_views.remove(&peerid);
		}
		NetworkBridgeEvent::PeerViewChange(peerid, view) => {
			handle_peer_view_change(ctx, state, peerid, view, metrics).await;
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			handle_our_view_change(ctx, keystore, state, view, metrics).await?;
		}
		NetworkBridgeEvent::PeerMessage(remote, msg) => {
			let gossiped_availability = match msg {
				protocol_v1::AvailabilityDistributionMessage::Chunk(candidate_hash, chunk) => {
					AvailabilityGossipMessage {
						candidate_hash,
						erasure_chunk: chunk,
					}
				}
			};

			process_incoming_peer_message(ctx, state, remote, gossiped_availability, metrics)
				.await?;
		}
	}
	Ok(())
}

/// Handle the changes necessary when our view changes.
#[tracing::instrument(level = "trace", skip(ctx, keystore, metrics), fields(subsystem = LOG_TARGET))]
async fn handle_our_view_change<Context>(
	ctx: &mut Context,
	keystore: &SyncCryptoStorePtr,
	state: &mut ProtocolState,
	view: View,
	metrics: &Metrics,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let _timer = metrics.time_handle_our_view_change();

	let old_view = std::mem::replace(&mut state.view, view);

	// needed due to borrow rules
	let view = state.view.clone();

	// add all the relay parents and fill the cache
	for added in view.difference(&old_view) {
		let validators = query_validators(ctx, *added).await?;
		let validator_index = obtain_our_validator_index(&validators, keystore.clone()).await;
		state
			.add_relay_parent(ctx, *added, validators, validator_index)
			.await?;
	}

	// handle all candidates
	for (candidate_hash, _receipt) in state.cached_live_candidates_unioned(view.difference(&old_view)) {
		let per_candidate = state.per_candidate.entry(candidate_hash).or_default();

		// assure the node has the validator role
		if per_candidate.validator_index.is_none() {
			continue;
		};

		// check if the availability is present in the store exists
		if !query_data_availability(ctx, candidate_hash).await? {
			continue;
		}

		let validator_count = per_candidate.validators.len();

		// obtain interested peers in the candidate hash
		let peers: Vec<PeerId> = state
			.peer_views
			.clone()
			.into_iter()
			.filter(|(_peer, view)| {
				// collect all direct interests of a peer w/o ancestors
				state
					.cached_live_candidates_unioned(view.0.iter())
					.contains_key(&candidate_hash)
			})
			.map(|(peer, _view)| peer.clone())
			.collect();

		// distribute all erasure messages to interested peers
		for chunk_index in 0u32..(validator_count as u32) {
			// only the peers which did not receive this particular erasure chunk
			let per_candidate = state.per_candidate.entry(candidate_hash).or_default();

			// obtain the chunks from the cache, if not fallback
			// and query the availability store
			let message_id = (candidate_hash, chunk_index);
			let erasure_chunk = if let Some(message) = per_candidate.message_vault.get(&chunk_index) {
				message.erasure_chunk.clone()
			} else if let Some(erasure_chunk) = query_chunk(ctx, candidate_hash, chunk_index as ValidatorIndex).await? {
				erasure_chunk
			} else {
				continue;
			};

			debug_assert_eq!(erasure_chunk.index, chunk_index);

			let peers = peers
				.iter()
				.filter(|peer| per_candidate.message_required_by_peer(peer, &message_id))
				.cloned()
				.collect::<Vec<_>>();
			let message = AvailabilityGossipMessage {
				candidate_hash,
				erasure_chunk,
			};

			send_tracked_gossip_message_to_peers(ctx, per_candidate, metrics, peers, message).await;
		}
	}

	// cleanup the removed relay parents and their states
	let removed = old_view.difference(&view).collect::<Vec<_>>();
	for removed in removed {
		state.remove_relay_parent(&removed);
	}
	Ok(())
}

#[inline(always)]
async fn send_tracked_gossip_message_to_peers<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	metrics: &Metrics,
	peers: Vec<PeerId>,
	message: AvailabilityGossipMessage,
)
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	send_tracked_gossip_messages_to_peers(ctx, per_candidate, metrics, peers, iter::once(message)).await
}

#[inline(always)]
async fn send_tracked_gossip_messages_to_peer<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	metrics: &Metrics,
	peer: PeerId,
	message_iter: impl IntoIterator<Item = AvailabilityGossipMessage>,
)
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	send_tracked_gossip_messages_to_peers(ctx, per_candidate, metrics, vec![peer], message_iter).await
}

#[tracing::instrument(level = "trace", skip(ctx, metrics, message_iter), fields(subsystem = LOG_TARGET))]
async fn send_tracked_gossip_messages_to_peers<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	metrics: &Metrics,
	peers: Vec<PeerId>,
	message_iter: impl IntoIterator<Item = AvailabilityGossipMessage>,
)
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	if peers.is_empty() {
		return;
	}
	for message in message_iter {
		for peer in peers.iter() {
			let message_id = (message.candidate_hash, message.erasure_chunk.index);
			per_candidate
				.sent_messages
				.entry(peer.clone())
				.or_default()
				.insert(message_id);
		}

		per_candidate
			.message_vault
			.insert(message.erasure_chunk.index, message.clone());

		let wire_message = protocol_v1::AvailabilityDistributionMessage::Chunk(
			message.candidate_hash,
			message.erasure_chunk,
		);

		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendValidationMessage(
				peers.clone(),
				protocol_v1::ValidationProtocol::AvailabilityDistribution(wire_message),
			),
		))
		.await;

		metrics.on_chunk_distributed();
	}
}

// Send the difference between two views which were not sent
// to that particular peer.
#[tracing::instrument(level = "trace", skip(ctx, metrics), fields(subsystem = LOG_TARGET))]
async fn handle_peer_view_change<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	view: View,
	metrics: &Metrics,
)
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let current = state.peer_views.entry(origin.clone()).or_default();

	let added: Vec<Hash> = view.difference(&*current).cloned().collect();

	*current = view;

	// only contains the intersection of what we are interested and
	// the union of all relay parent's candidates.
	let added_candidates = state.cached_live_candidates_unioned(added.iter());

	// Send all messages we've seen before and the peer is now interested
	// in to that peer.

	for (candidate_hash, _receipt) in added_candidates {
		let per_candidate = state.per_candidate.entry(candidate_hash).or_default();

		// obtain the relevant chunk indices not sent yet
		let messages = ((0 as ValidatorIndex)..(per_candidate.validators.len() as ValidatorIndex))
			.into_iter()
			.filter_map(|erasure_chunk_index: ValidatorIndex| {
				let message_id = (candidate_hash, erasure_chunk_index);

				// try to pick up the message from the message vault
				// so we send as much as we have
				per_candidate
					.message_vault
					.get(&erasure_chunk_index)
					.filter(|_| per_candidate.message_required_by_peer(&origin, &message_id))
			})
			.cloned()
			.collect::<HashSet<_>>();

		send_tracked_gossip_messages_to_peer(ctx, per_candidate, metrics, origin.clone(), messages).await;
	}
}

/// Obtain the first key which has a signing key.
/// Returns the index within the validator set as `ValidatorIndex`, if there exists one,
/// otherwise, `None` is returned.
async fn obtain_our_validator_index(
	validators: &[ValidatorId],
	keystore: SyncCryptoStorePtr,
) -> Option<ValidatorIndex> {
	for (idx, validator) in validators.iter().enumerate() {
		if CryptoStore::has_keys(
			&*keystore,
			&[(validator.to_raw_vec(), PARACHAIN_KEY_TYPE_ID)],
		)
		.await
		{
			return Some(idx as ValidatorIndex);
		}
	}
	None
}

/// Handle an incoming message from a peer.
#[tracing::instrument(level = "trace", skip(ctx, metrics), fields(subsystem = LOG_TARGET))]
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	message: AvailabilityGossipMessage,
	metrics: &Metrics,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let _timer = metrics.time_process_incoming_peer_message();

	// obtain the set of candidates we are interested in based on our current view
	let live_candidates = state.cached_live_candidates_unioned(state.view.0.iter());

	// check if the candidate is of interest
	let live_candidate = if let Some(live_candidate) = live_candidates.get(&message.candidate_hash) {
		live_candidate
	} else {
		modify_reputation(ctx, origin, COST_NOT_A_LIVE_CANDIDATE).await;
		return Ok(());
	};

	// check the merkle proof
	let root = &live_candidate.descriptor.erasure_root;
	let anticipated_hash = if let Ok(hash) = branch_hash(
		root,
		&message.erasure_chunk.proof,
		message.erasure_chunk.index as usize,
	) {
		hash
	} else {
		modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
		return Ok(());
	};

	let erasure_chunk_hash = BlakeTwo256::hash(&message.erasure_chunk.chunk);
	if anticipated_hash != erasure_chunk_hash {
		modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
		return Ok(());
	}

	// an internal unique identifier of this message
	let message_id = (message.candidate_hash, message.erasure_chunk.index);

	{
		let per_candidate = state.per_candidate.entry(message_id.0.clone()).or_default();

		// check if this particular erasure chunk was already sent by that peer before
		{
			let received_set = per_candidate
				.received_messages
				.entry(origin.clone())
				.or_default();
			if received_set.contains(&message_id) {
				modify_reputation(ctx, origin, COST_PEER_DUPLICATE_MESSAGE).await;
				return Ok(());
			} else {
				received_set.insert(message_id.clone());
			}
		}

		// insert into known messages and change reputation
		if per_candidate
			.message_vault
			.insert(message_id.1, message.clone())
			.is_some()
		{
			modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE).await;
		} else {
			modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE_FIRST).await;

			// save the chunk for our index
			if let Some(validator_index) = per_candidate.validator_index {
				if message.erasure_chunk.index == validator_index {
					if let Err(_e) = store_chunk(
						ctx,
						message.candidate_hash.clone(),
						live_candidate.descriptor.relay_parent.clone(),
						message.erasure_chunk.index,
						message.erasure_chunk.clone(),
					)
					.await?
					{
						tracing::warn!(
							target: LOG_TARGET,
							"Failed to store erasure chunk to availability store"
						);
					}
				}
			}
		};
	}
	// condense the peers to the peers with interest on the candidate
	let peers = state
		.peer_views
		.clone()
		.into_iter()
		.filter(|(_peer, view)| {
			// peers view must contain the candidate hash too
			state
				.cached_live_candidates_unioned(view.0.iter())
				.contains_key(&message_id.0)
		})
		.map(|(peer, _)| -> PeerId { peer.clone() })
		.collect::<Vec<_>>();

	let per_candidate = state.per_candidate.entry(message_id.0.clone()).or_default();

	let peers = peers
		.into_iter()
		.filter(|peer| per_candidate.message_required_by_peer(peer, &message_id))
		.collect::<Vec<_>>();

	// gossip that message to interested peers
	send_tracked_gossip_message_to_peers(ctx, per_candidate, metrics, peers, message).await;
	Ok(())
}

/// The bitfield distribution subsystem.
pub struct AvailabilityDistributionSubsystem {
	/// Pointer to a keystore, which is required for determining this nodes validator index.
	keystore: SyncCryptoStorePtr,
	/// Prometheus metrics.
	metrics: Metrics,
}

impl AvailabilityDistributionSubsystem {
	/// Number of ancestors to keep around for the relay-chain heads.
	const K: usize = 3;

	/// Create a new instance of the availability distribution.
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		Self { keystore, metrics }
	}

	/// Start processing work as passed on from the Overseer.
	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run<Context>(self, mut ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		// work: process incoming messages from the overseer.
		let mut state = ProtocolState::default();
		loop {
			let message = ctx
				.recv()
				.await
				.map_err(|e| Error::IncomingMessageChannel(e))?;
			match message {
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::NetworkBridgeUpdateV1(event),
				} => {
					if let Err(e) = handle_network_msg(
						&mut ctx,
						&self.keystore.clone(),
						&mut state,
						&self.metrics,
						event,
					)
					.await
					{
						tracing::warn!(
							target: LOG_TARGET,
							err = ?e,
							"Failed to handle incoming network messages",
						);
					}
				}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: _,
					deactivated: _,
				})) => {
					// handled at view change
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return Ok(());
				}
			}
		}
	}
}

impl<Context> Subsystem<Context> for AvailabilityDistributionSubsystem
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("availability-distribution", e))
			.boxed();

		SpawnedSubsystem {
			name: "availability-distribution-subsystem",
			future,
		}
	}
}

/// Obtain all live candidates based on an iterator of relay heads.
#[tracing::instrument(level = "trace", skip(ctx, relay_parents), fields(subsystem = LOG_TARGET))]
async fn query_live_candidates_without_ancestors<Context>(
	ctx: &mut Context,
	relay_parents: impl IntoIterator<Item = Hash>,
) -> Result<HashSet<CommittedCandidateReceipt>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let iter = relay_parents.into_iter();
	let hint = iter.size_hint();

	let mut live_candidates = HashSet::with_capacity(hint.1.unwrap_or(hint.0));
	for relay_parent in iter {
		let paras = query_para_ids(ctx, relay_parent).await?;
		for para in paras {
			if let Some(ccr) = query_pending_availability(ctx, relay_parent, para).await? {
				live_candidates.insert(ccr);
			}
		}
	}
	Ok(live_candidates)
}

/// Obtain all live candidates based on an iterator or relay heads including `k` ancestors.
///
/// Relay parent.
#[tracing::instrument(level = "trace", skip(ctx, relay_parents), fields(subsystem = LOG_TARGET))]
async fn query_live_candidates<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	relay_parents: impl IntoIterator<Item = Hash>,
) -> Result<HashMap<Hash, (CandidateHash, CommittedCandidateReceipt)>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let iter = relay_parents.into_iter();
	let hint = iter.size_hint();

	let capacity = hint.1.unwrap_or(hint.0) * (1 + AvailabilityDistributionSubsystem::K);
	let mut live_candidates =
		HashMap::<Hash, (CandidateHash, CommittedCandidateReceipt)>::with_capacity(capacity);

	for relay_parent in iter {
		// register one of relay parents (not the ancestors)
		let mut ancestors = query_up_to_k_ancestors_in_same_session(
			ctx,
			relay_parent,
			AvailabilityDistributionSubsystem::K,
		)
		.await?;

		ancestors.push(relay_parent);

		// ancestors might overlap, so check the cache too
		let unknown = ancestors
			.into_iter()
			.filter(|relay_parent_or_ancestor| {
				// use the ones which we pulled before
				// but keep the unknown relay parents
				state
					.receipts
					.get(relay_parent_or_ancestor)
					.and_then(|receipts| {
						// directly extend the live_candidates with the cached value
						live_candidates.extend(receipts.into_iter().map(
							|(receipt_hash, receipt)| {
								(relay_parent, (receipt_hash.clone(), receipt.clone()))
							},
						));
						Some(())
					})
					.is_none()
			})
			.collect::<Vec<_>>();

		// query the ones that were not present in the receipts cache
		let receipts = query_live_candidates_without_ancestors(ctx, unknown.clone()).await?;
		live_candidates.extend(
			unknown.into_iter().zip(
				receipts
					.into_iter()
					.map(|receipt| (receipt.hash(), receipt)),
			),
		);
	}
	Ok(live_candidates)
}

/// Query all para IDs.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_para_ids<Context>(ctx: &mut Context, relay_parent: Hash) -> Result<Vec<ParaId>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	)))
	.await;

	let all_para_ids: Vec<_> = rx
		.await
		.map_err(|e| Error::AvailabilityCoresResponseChannel(e))?
		.map_err(|e| Error::AvailabilityCores(e))?;

	let occupied_para_ids = all_para_ids
		.into_iter()
		.filter_map(|core_state| {
			if let CoreState::Occupied(occupied) = core_state {
				Some(occupied.para_id)
			} else {
				None
			}
		})
		.collect();
	Ok(occupied_para_ids)
}

/// Modify the reputation of a peer based on its behavior.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn modify_reputation<Context>(ctx: &mut Context, peer: PeerId, rep: Rep)
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	tracing::trace!(
		target: LOG_TARGET,
		rep = ?rep,
		peer_id = ?peer,
		"Reputation change for peer",
	);
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	)).await;
}

/// Query the proof of validity for a particular candidate hash.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_data_availability<Context>(ctx: &mut Context, candidate_hash: CandidateHash) -> Result<bool>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryDataAvailability(candidate_hash, tx),
	)).await;

	rx.await
		.map_err(|e| Error::QueryAvailabilityResponseChannel(e))
}

#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_chunk<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
	validator_index: ValidatorIndex,
) -> Result<Option<ErasureChunk>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx),
	)).await;

	rx.await.map_err(|e| Error::QueryChunkResponseChannel(e))
}

#[tracing::instrument(level = "trace", skip(ctx, erasure_chunk), fields(subsystem = LOG_TARGET))]
async fn store_chunk<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
	relay_parent: Hash,
	validator_index: ValidatorIndex,
	erasure_chunk: ErasureChunk,
) -> Result<std::result::Result<(), ()>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			validator_index,
			chunk: erasure_chunk,
			tx,
		}
	)).await;

	rx.await.map_err(|e| Error::StoreChunkResponseChannel(e))
}

/// Request the head data for a particular para.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_pending_availability<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	para: ParaId,
) -> Result<Option<CommittedCandidateReceipt>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::CandidatePendingAvailability(para, tx),
	))).await;

	rx.await
		.map_err(|e| Error::QueryPendingAvailabilityResponseChannel(e))?
		.map_err(|e| Error::QueryPendingAvailability(e))
}

/// Query the validator set.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_validators<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<Vec<ValidatorId>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::Validators(tx),
	));

	ctx.send_message(query_validators)
		.await;
	rx.await
		.map_err(|e| Error::QueryValidatorsResponseChannel(e))?
		.map_err(|e| Error::QueryValidators(e))
}

/// Query the hash of the `K` ancestors
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_k_ancestors<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	k: usize,
) -> Result<Vec<Hash>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_ancestors = AllMessages::ChainApi(ChainApiMessage::Ancestors {
		hash: relay_parent,
		k,
		response_channel: tx,
	});

	ctx.send_message(query_ancestors)
		.await;
	rx.await
		.map_err(|e| Error::QueryAncestorsResponseChannel(e))?
		.map_err(|e| Error::QueryAncestors(e))
}

/// Query the session index of a relay parent
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_session_index_for_child<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<SessionIndex>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_session_idx_for_child = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::SessionIndexForChild(tx),
	));

	ctx.send_message(query_session_idx_for_child)
		.await;
	rx.await
		.map_err(|e| Error::QuerySessionResponseChannel(e))?
		.map_err(|e| Error::QuerySession(e))
}

/// Queries up to k ancestors with the constraints of equiv session
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_up_to_k_ancestors_in_same_session<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	k: usize,
) -> Result<Vec<Hash>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	// k + 1 since we always query the child's session index
	// ordering is [parent, grandparent, greatgrandparent, greatgreatgrandparent, ...]
	let ancestors = query_k_ancestors(ctx, relay_parent, k + 1).await?;
	let desired_session = query_session_index_for_child(ctx, relay_parent).await?;
	// we would only need `ancestors.len() - 1`, but the one extra could avoid a re-alloc
	// if the consumer wants to push the `relay_parent` onto it too and does not hurt otherwise
	let mut acc = Vec::with_capacity(ancestors.len());

	// iterate from youngest to oldest
	let mut iter = ancestors.into_iter().peekable();

	while let Some(ancestor) = iter.next() {
		if let Some(ancestor_parent) = iter.peek() {
			let session = query_session_index_for_child(ctx, *ancestor_parent).await?;
			if session != desired_session {
				break;
			}
			acc.push(ancestor);
		} else {
			// either ended up at genesis or the blocks were
			// already pruned
			break;
		}
	}

	debug_assert!(acc.len() <= k);
	Ok(acc)
}

#[derive(Clone)]
struct MetricsInner {
	gossipped_availability_chunks: prometheus::Counter<prometheus::U64>,
	handle_our_view_change: prometheus::Histogram,
	process_incoming_peer_message: prometheus::Histogram,
}

/// Availability Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_chunk_distributed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.gossipped_availability_chunks.inc();
		}
	}

	/// Provide a timer for `handle_our_view_change` which observes on drop.
	fn time_handle_our_view_change(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_our_view_change.start_timer())
	}

	/// Provide a timer for `process_incoming_peer_message` which observes on drop.
	fn time_process_incoming_peer_message(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_incoming_peer_message.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(
		registry: &prometheus::Registry,
	) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			gossipped_availability_chunks: prometheus::register(
				prometheus::Counter::new(
					"parachain_gossipped_availability_chunks_total",
					"Number of availability chunks gossipped to other peers.",
				)?,
				registry,
			)?,
			handle_our_view_change: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_availability_distribution_handle_our_view_change",
						"Time spent within `availability_distribution::handle_our_view_change`",
					)
				)?,
				registry,
			)?,
			process_incoming_peer_message: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_availability_distribution_process_incoming_peer_message",
						"Time spent within `availability_distribution::process_incoming_peer_message`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
