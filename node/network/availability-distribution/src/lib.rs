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
	v1 as protocol_v1, PeerId, View, OurView, UnifiedReputationChange as Rep,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_primitives::v1::{
	BlakeTwo256, CoreState, ErasureChunk, Hash, HashT,
	SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID, CandidateHash,
	CandidateDescriptor,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
	NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest, NetworkBridgeEvent
};
use polkadot_subsystem::{
	jaeger, errors::{ChainApiError, RuntimeApiError}, PerLeafSpan, Stage,
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError,
};
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::iter;
use thiserror::Error;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "availability_distribution";

#[derive(Debug, Error)]
enum Error {
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

const COST_MERKLE_PROOF_INVALID: Rep = Rep::CostMinor("Merkle proof was invalid");
const COST_NOT_A_LIVE_CANDIDATE: Rep = Rep::CostMinor("Candidate is not live");
const COST_PEER_DUPLICATE_MESSAGE: Rep = Rep::CostMajorRepeated("Peer sent identical messages");
const BENEFIT_VALID_MESSAGE_FIRST: Rep = Rep::BenefitMinorFirst("Valid message with new information");
const BENEFIT_VALID_MESSAGE: Rep = Rep::BenefitMinor("Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AvailabilityGossipMessage {
	/// Anchor hash of the candidate the `ErasureChunk` is associated to.
	pub candidate_hash: CandidateHash,
	/// The erasure chunk, a encoded information part of `AvailabilityData`.
	pub erasure_chunk: ErasureChunk,
}

impl From<AvailabilityGossipMessage> for protocol_v1::AvailabilityDistributionMessage {
	fn from(message: AvailabilityGossipMessage) -> Self {
		Self::Chunk(message.candidate_hash, message.erasure_chunk)
	}
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Debug, Default)]
struct ProtocolState {
	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our own view.
	view: OurView,

	/// Caches a mapping of relay parents or ancestor to live candidate hashes.
	/// Allows fast intersection of live candidates with views and consecutive unioning.
	/// Maps relay parent / ancestor -> candidate hashes.
	live_under: HashMap<Hash, HashSet<CandidateHash>>,

	/// Track things needed to start and stop work on a particular relay parent.
	per_relay_parent: HashMap<Hash, PerRelayParent>,

	/// Track data that is specific to a candidate.
	per_candidate: HashMap<CandidateHash, PerCandidate>,
}

#[derive(Debug)]
struct PerCandidate {
	/// A Candidate and a set of known erasure chunks in form of messages to be gossiped / distributed if the peer view wants that.
	/// This is _across_ peers and not specific to a particular one.
	/// candidate hash + erasure chunk index -> gossip message
	message_vault: HashMap<u32, AvailabilityGossipMessage>,

	/// Track received erasure chunk indices per peer.
	received_messages: HashMap<PeerId, HashSet<ValidatorIndex>>,

	/// Track sent erasure chunk indices per peer.
	sent_messages: HashMap<PeerId, HashSet<ValidatorIndex>>,

	/// The set of validators.
	validators: Vec<ValidatorId>,

	/// If this node is a validator, note the index in the validator set.
	validator_index: Option<ValidatorIndex>,

	/// The descriptor of this candidate.
	descriptor: CandidateDescriptor,

	/// The set of relay chain blocks this appears to be live in.
	live_in: HashSet<Hash>,

	/// A Jaeger span relating to this candidate.
	span: jaeger::Span,
}

impl PerCandidate {
	/// Returns `true` iff the given `validator_index` is required by the given `peer`.
	fn message_required_by_peer(&self, peer: &PeerId, validator_index: ValidatorIndex) -> bool {
		self.received_messages.get(peer).map(|v| !v.contains(&validator_index)).unwrap_or(true)
			&& self.sent_messages.get(peer).map(|v| !v.contains(&validator_index)).unwrap_or(true)
	}

	/// Add a chunk to the message vault. Overwrites anything that was already present.
	fn add_message(&mut self, chunk_index: u32, message: AvailabilityGossipMessage) {
		let _ = self.message_vault.insert(chunk_index, message);
	}

	/// Clean up the span if we've got our own chunk.
	fn drop_span_after_own_availability(&mut self) {
		if let Some(validator_index) = self.validator_index {
			if self.message_vault.contains_key(&validator_index) {
				self.span = jaeger::Span::Disabled;
			}
		}
	}
}

#[derive(Debug)]
struct PerRelayParent {
	/// Set of `K` ancestors for this relay parent.
	ancestors: Vec<Hash>,
	/// Live candidates, according to this relay parent.
	live_candidates: HashSet<CandidateHash>,
	/// The span that belongs to this relay parent.
	span: PerLeafSpan,
}

impl ProtocolState {
	/// Unionize all live candidate hashes of the given relay parents and their recent
	/// ancestors.
	///
	/// Ignores all non existent relay parents, so this can be used directly with a peers view.
	/// Returns a set of candidate hashes.
	#[tracing::instrument(level = "trace", skip(relay_parents), fields(subsystem = LOG_TARGET))]
	fn cached_live_candidates_unioned<'a>(
		&'a self,
		relay_parents: impl IntoIterator<Item = &'a Hash> + 'a,
	) -> HashSet<CandidateHash> {
		cached_live_candidates_unioned(
			&self.per_relay_parent,
			relay_parents
		)
	}

	#[tracing::instrument(level = "trace", skip(candidates, span), fields(subsystem = LOG_TARGET))]
	fn add_relay_parent(
		&mut self,
		relay_parent: Hash,
		validators: Vec<ValidatorId>,
		validator_index: Option<ValidatorIndex>,
		candidates: HashMap<CandidateHash, FetchedLiveCandidate>,
		ancestors: Vec<Hash>,
		span: PerLeafSpan,
	) {
		let per_relay_parent = self.per_relay_parent.entry(relay_parent).or_insert_with(|| PerRelayParent {
			span,
			ancestors,
			live_candidates: candidates.keys().cloned().collect(),
		});

		// register the relation of relay_parent to candidate..
		for (receipt_hash, fetched) in candidates {
			let candidate_entry = match self.per_candidate.entry(receipt_hash) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => {
					if let FetchedLiveCandidate::Fresh(descriptor) = fetched {
						e.insert(PerCandidate {
							message_vault: HashMap::new(),
							received_messages: HashMap::new(),
							sent_messages: HashMap::new(),
							validators: validators.clone(),
							validator_index,
							descriptor,
							live_in: HashSet::new(),
							span: if validator_index.is_some() {
								jaeger::candidate_hash_span(&receipt_hash, "pending-availability")
							} else {
								jaeger::Span::Disabled
							},
						})
					} else {
						tracing::warn!(target: LOG_TARGET, "No `per_candidate` but not fresh. logic error");
						continue;
					}
				}
			};

			// Create some span that will make it able to switch between the candidate and relay parent span.
			let span = per_relay_parent.span.child_builder("live-candidate")
				.with_candidate(&receipt_hash)
				.with_stage(Stage::AvailabilityDistribution)
				.build();
			candidate_entry.span.add_follows_from(&span);
			candidate_entry.live_in.insert(relay_parent);
		}
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn remove_relay_parent(&mut self, relay_parent: &Hash) {
		if let Some(per_relay_parent) = self.per_relay_parent.remove(relay_parent) {
			for candidate_hash in per_relay_parent.live_candidates {
				// Prune the candidate if this was the last member of our view
				// to consider it live (including its ancestors).
				if let Entry::Occupied(mut occ) = self.per_candidate.entry(candidate_hash) {
					occ.get_mut().live_in.remove(relay_parent);
					if occ.get().live_in.is_empty() {
						occ.remove();
					}
				}
			}
		}
	}

	/// Removes all entries from live_under which aren't referenced in the ancestry of
	/// one of our live relay-chain heads.
	fn clean_up_live_under_cache(&mut self) {
		let extended_view: HashSet<_> = self.per_relay_parent.iter()
			.map(|(r_hash, v)| v.ancestors.iter().cloned().chain(iter::once(*r_hash)))
			.flatten()
			.collect();

		self.live_under.retain(|ancestor_hash, _| extended_view.contains(ancestor_hash));
	}
}

fn cached_live_candidates_unioned<'a>(
	per_relay_parent: &'a HashMap<Hash, PerRelayParent>,
	relay_parents: impl IntoIterator<Item = &'a Hash> + 'a,
) -> HashSet<CandidateHash> {
	relay_parents
		.into_iter()
		.filter_map(|r| per_relay_parent.get(r))
		.map(|per_relay_parent| per_relay_parent.live_candidates.iter().cloned())
		.flatten()
		.collect()
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
	view: OurView,
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
	for (added, span) in view.span_per_head().iter().filter(|v| !old_view.contains(&v.0)) {
		let span = PerLeafSpan::new(span.clone(), "availability-distribution");

		let validators = query_validators(ctx, *added).await?;
		let validator_index = obtain_our_validator_index(&validators, keystore.clone()).await;
		let (candidates, ancestors)
			= query_live_candidates(ctx, &mut state.live_under, *added).await?;

		state.add_relay_parent(
			*added,
			validators,
			validator_index,
			candidates,
			ancestors,
			span,
		);
	}

	// handle all candidates
	let mut messages_out = Vec::new();
	for candidate_hash in state.cached_live_candidates_unioned(view.difference(&old_view)) {
		// If we are not a validator for this candidate, let's skip it.
		match state.per_candidate.get(&candidate_hash) {
			None => continue,
			Some(c) if c.validator_index.is_none() => continue,
			Some(_) => {},
		};

		// check if the availability is present in the store exists
		if !query_data_availability(ctx, candidate_hash).await? {
			continue;
		}

		// obtain interested peers in the candidate hash
		let peers: Vec<PeerId> = state
			.peer_views
			.clone()
			.into_iter()
			.filter(|(_peer, view)| {
				// collect all direct interests of a peer w/o ancestors
				state
					.cached_live_candidates_unioned(view.heads.iter())
					.contains(&candidate_hash)
			})
			.map(|(peer, _view)| peer.clone())
			.collect();

		let per_candidate = state.per_candidate.get_mut(&candidate_hash)
			.expect("existence checked above; qed");

		let validator_count = per_candidate.validators.len();

		// distribute all erasure messages to interested peers
		for chunk_index in 0u32..(validator_count as u32) {
			let _span = per_candidate.span
				.child_builder("load-and-distribute")
				.with_candidate(&candidate_hash)
				.with_chunk_index(chunk_index)
				.build();

			let message = if let Some(message) = per_candidate.message_vault.get(&chunk_index) {
				tracing::trace!(
					target: LOG_TARGET,
					%chunk_index,
					?candidate_hash,
					"Retrieved chunk from message vault",
				);
				message.clone()
			} else if let Some(erasure_chunk) = query_chunk(ctx, candidate_hash, chunk_index as ValidatorIndex).await? {
				tracing::trace!(
					target: LOG_TARGET,
					%chunk_index,
					?candidate_hash,
					"Retrieved chunk from availability storage",
				);

				let msg = AvailabilityGossipMessage {
					candidate_hash,
					erasure_chunk,
				};

				per_candidate.add_message(chunk_index, msg.clone());

				msg
			} else {
				tracing::error!(
					target: LOG_TARGET,
					%chunk_index,
					?candidate_hash,
					"Availability store reported that we have the availability data, but we could not retrieve a chunk of it!",
				);
				continue;
			};

			debug_assert_eq!(message.erasure_chunk.index, chunk_index);

			let peers = peers
				.iter()
				.filter(|peer| per_candidate.message_required_by_peer(peer, chunk_index))
				.cloned()
				.collect::<Vec<_>>();

			add_tracked_messages_to_batch(&mut messages_out, per_candidate, metrics, peers, iter::once(message));
		}

		// traces are better if we wait until the loop is done to drop.
		per_candidate.drop_span_after_own_availability();
	}

	// send all batched messages out.
	send_batch_to_network(ctx, messages_out).await;

	// cleanup the removed relay parents and their states
	old_view.difference(&view).for_each(|r| state.remove_relay_parent(r));
	state.clean_up_live_under_cache();

	Ok(())
}

// After this function is invoked, the state reflects the messages as having been sent to a peer.
#[tracing::instrument(level = "trace", skip(batch, metrics, message_iter), fields(subsystem = LOG_TARGET))]
fn add_tracked_messages_to_batch(
	batch: &mut Vec<(Vec<PeerId>, protocol_v1::ValidationProtocol)>,
	per_candidate: &mut PerCandidate,
	metrics: &Metrics,
	peers: Vec<PeerId>,
	message_iter: impl IntoIterator<Item = AvailabilityGossipMessage>,
) {
	for message in message_iter {
		for peer in peers.iter() {
			per_candidate
				.sent_messages
				.entry(peer.clone())
				.or_default()
				.insert(message.erasure_chunk.index);
		}

		if !peers.is_empty() {
			batch.push((
				peers.clone(),
				protocol_v1::ValidationProtocol::AvailabilityDistribution(message.into()),
			));

			metrics.on_chunk_distributed();
		}
	}
}

async fn send_batch_to_network(
	ctx: &mut impl SubsystemContext,
	batch: Vec<(Vec<PeerId>, protocol_v1::ValidationProtocol)>,
) {
	if !batch.is_empty() {
		ctx.send_message(NetworkBridgeMessage::SendValidationMessages(batch).into()).await
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

	if added.is_empty() {
		return
	}

	// only contains the intersection of what we are interested and
	// the union of all relay parent's candidates.
	let added_candidates = state.cached_live_candidates_unioned(added.iter());

	// Send all messages we've seen before and the peer is now interested in.
	let mut batch = Vec::new();
	for candidate_hash in added_candidates {
		let per_candidate = match state.per_candidate.get_mut(&candidate_hash) {
			Some(p) => p,
			None => continue,
		};

		// obtain the relevant chunk indices not sent yet
		let messages = ((0 as ValidatorIndex)..(per_candidate.validators.len() as ValidatorIndex))
			.into_iter()
			.filter_map(|erasure_chunk_index: ValidatorIndex| {
				// try to pick up the message from the message vault
				// so we send as much as we have
				per_candidate
					.message_vault
					.get(&erasure_chunk_index)
					.filter(|_| per_candidate.message_required_by_peer(&origin, erasure_chunk_index))
			})
			.cloned()
			.collect::<HashSet<_>>();

		add_tracked_messages_to_batch(&mut batch, per_candidate, metrics, vec![origin.clone()], messages);
	}

	send_batch_to_network(ctx, batch).await;
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
	let live_candidates = state.cached_live_candidates_unioned(state.view.heads.iter());

	// check if the candidate is of interest
	let candidate_hash = message.candidate_hash;
	let candidate_entry = if live_candidates.contains(&candidate_hash) {
		state.per_candidate
			.get_mut(&candidate_hash)
			.expect("All live candidates are contained in per_candidate; qed")
	} else {
		tracing::trace!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			peer = %origin,
			"Peer send not live candidate",
		);
		modify_reputation(ctx, origin, COST_NOT_A_LIVE_CANDIDATE).await;
		return Ok(())
	};

	// Handle a duplicate before doing expensive checks.
	if let Some(existing) = candidate_entry.message_vault.get(&message.erasure_chunk.index) {
		let span = candidate_entry.span.child_with_candidate("handle-duplicate", &candidate_hash);
		// check if this particular erasure chunk was already sent by that peer before
		{
			let _span = span.child_with_candidate("check-entry", &candidate_hash);
			let received_set = candidate_entry
				.received_messages
				.entry(origin.clone())
				.or_default();

			if !received_set.insert(message.erasure_chunk.index) {
				modify_reputation(ctx, origin, COST_PEER_DUPLICATE_MESSAGE).await;
				return Ok(());
			}
		}

		// check that the message content matches what we have already before rewarding
		// the peer.
		{
			let _span = span.child_with_candidate("check-accurate", &candidate_hash);
			if existing == &message {
				modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE).await;
			} else {
				modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
			}
		}

		return Ok(());
	}

	let span = candidate_entry.span
			.child_builder("process-new-chunk")
			.with_candidate(&candidate_hash)
			.with_peer_id(&origin)
			.build();

	// check the merkle proof against the erasure root in the candidate descriptor.
	let anticipated_hash = {
		let _span = span.child_with_candidate("check-merkle-root", &candidate_hash);
		match branch_hash(
			&candidate_entry.descriptor.erasure_root,
			&message.erasure_chunk.proof,
			message.erasure_chunk.index as usize,
		) {
			Ok(hash) => hash,
			Err(e) => {
				tracing::trace!(
					target: LOG_TARGET,
					candidate_hash = ?message.candidate_hash,
					peer = %origin,
					error = ?e,
					"Failed to calculate chunk merkle proof",
				);
				modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
				return Ok(());
			},
		}
	};

	{
		let _span = span.child("check-chunk-hash");
		let erasure_chunk_hash = BlakeTwo256::hash(&message.erasure_chunk.chunk);
		if anticipated_hash != erasure_chunk_hash {
			tracing::trace!(
				target: LOG_TARGET,
				candidate_hash = ?message.candidate_hash,
				peer = %origin,
				"Peer sent chunk with invalid merkle proof",
			);
			modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
			return Ok(());
		}
	}

	{
		// insert into known messages and change reputation. we've guaranteed
		// above that the message vault doesn't contain any message under this
		// chunk index already.

		candidate_entry
				.received_messages
				.entry(origin.clone())
				.or_default()
				.insert(message.erasure_chunk.index);

		modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE_FIRST).await;

		// save the chunk for our index
		if Some(message.erasure_chunk.index) == candidate_entry.validator_index {
			let _span = span.child("store-our-chunk");
			if store_chunk(
				ctx,
				message.candidate_hash,
				candidate_entry.descriptor.relay_parent,
				message.erasure_chunk.index,
				message.erasure_chunk.clone(),
			).await?.is_err() {
				tracing::warn!(
					target: LOG_TARGET,
					"Failed to store erasure chunk to availability store"
				);
			}
		}

		candidate_entry.add_message(message.erasure_chunk.index, message.clone());
		candidate_entry.drop_span_after_own_availability();
	}

	// condense the peers to the peers with interest on the candidate
	let peers = {
		let _span = span.child("determine-recipient-peers");
		let per_relay_parent = &state.per_relay_parent;

		state
			.peer_views
			.clone()
			.into_iter()
			.filter(|(_, view)| {
				// peers view must contain the candidate hash too
				cached_live_candidates_unioned(
					per_relay_parent,
					view.heads.iter(),
				).contains(&message.candidate_hash)
			})
			.map(|(peer, _)| -> PeerId { peer.clone() })
			.filter(|peer| candidate_entry.message_required_by_peer(peer, message.erasure_chunk.index))
			.collect::<Vec<_>>()
	};

	drop(span);
	// gossip that message to interested peers
	let mut batch = Vec::new();
	add_tracked_messages_to_batch(&mut batch, candidate_entry, metrics, peers, iter::once(message));
	send_batch_to_network(ctx, batch).await;

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
	async fn run<Context>(self, ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		let mut state = ProtocolState {
			peer_views: HashMap::new(),
			view: Default::default(),
			live_under: HashMap::new(),
			per_relay_parent: HashMap::new(),
			per_candidate: HashMap::new(),
		};

		self.run_inner(ctx, &mut state).await
	}

	/// Start processing work.
	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run_inner<Context>(self, mut ctx: Context, state: &mut ProtocolState) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		// work: process incoming messages from the overseer.
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
						state,
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
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::AvailabilityFetchingRequest(_),
				} => {
					// TODO: Implement issue 2306:
					tracing::warn!(
						target: LOG_TARGET,
						"To be implemented, see: https://github.com/paritytech/polkadot/issues/2306 !",
					);
				}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: _,
					deactivated: _,
				})) => {
					// handled at view change
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {}
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

/// Metadata about a candidate that is part of the live_candidates set.
///
/// Those which were not present in a cache are "fresh" and have their candidate descriptor attached. This
/// information is propagated to the higher level where it can be used to create data entries. Cached candidates
/// already have entries associated with them, and thus don't need this metadata to be fetched.
#[derive(Debug)]
enum FetchedLiveCandidate {
	Cached,
	Fresh(CandidateDescriptor),
}

/// Obtain all live candidates for all given `relay_blocks`.
///
/// This returns a set of all candidate hashes pending availability within the state
/// of the explicitly referenced relay heads.
///
/// This also queries the provided `live_under` cache before reaching into the
/// runtime and updates it with the information learned.
#[tracing::instrument(level = "trace", skip(ctx, relay_blocks, live_under), fields(subsystem = LOG_TARGET))]
async fn query_pending_availability_at<Context>(
	ctx: &mut Context,
	relay_blocks: impl IntoIterator<Item = Hash>,
	live_under: &mut HashMap<Hash, HashSet<CandidateHash>>,
) -> Result<HashMap<CandidateHash, FetchedLiveCandidate>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let mut live_candidates = HashMap::new();

	// fetch and fill out cache for each of these
	for relay_parent in relay_blocks {
		let receipts_for = match live_under.entry(relay_parent) {
			Entry::Occupied(e) => {
				live_candidates.extend(
					e.get().iter().cloned().map(|c| (c, FetchedLiveCandidate::Cached))
				);
				continue
			},
			e => e.or_default(),
		};

		for (receipt_hash, descriptor) in query_pending_availability(ctx, relay_parent).await? {
			// unfortunately we have no good way of telling the candidate was
			// cached until now. But we don't clobber a `Cached` entry if there
			// is one already.
			live_candidates.entry(receipt_hash).or_insert(FetchedLiveCandidate::Fresh(descriptor));
			receipts_for.insert(receipt_hash);
		}
	}

	Ok(live_candidates)
}

/// Obtain all live candidates under a particular relay head. This implicitly includes
/// `K` ancestors of the head, such that the candidates pending availability in all of
/// the states of the head and the ancestors are unioned together to produce the
/// return type of this function. Each candidate hash is paired with information about
/// from where it was fetched.
///
/// This also updates all `live_under` cached by the protocol state and returns a list
/// of up to `K` ancestors of the relay-parent.
#[tracing::instrument(level = "trace", skip(ctx, live_under), fields(subsystem = LOG_TARGET))]
async fn query_live_candidates<Context>(
	ctx: &mut Context,
	live_under: &mut HashMap<Hash, HashSet<CandidateHash>>,
	relay_parent: Hash,
) -> Result<(HashMap<CandidateHash, FetchedLiveCandidate>, Vec<Hash>)>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	// register one of relay parents (not the ancestors)
	let ancestors = query_up_to_k_ancestors_in_same_session(
		ctx,
		relay_parent,
		AvailabilityDistributionSubsystem::K,
	)
	.await?;

	// query the ones that were not present in the live_under cache and add them
	// to it.
	let live_candidates = query_pending_availability_at(
		ctx,
		ancestors.iter().cloned().chain(iter::once(relay_parent)),
		live_under,
	).await?;

	Ok((live_candidates, ancestors))
}

/// Query all hashes and descriptors of candidates pending availability at a particular block.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_pending_availability<Context>(ctx: &mut Context, relay_parent: Hash)
	-> Result<Vec<(CandidateHash, CandidateDescriptor)>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	)))
	.await;

	let cores: Vec<_> = rx
		.await
		.map_err(|e| Error::AvailabilityCoresResponseChannel(e))?
		.map_err(|e| Error::AvailabilityCores(e))?;

	Ok(cores.into_iter()
		.filter_map(|core_state| if let CoreState::Occupied(occupied) = core_state {
			Some((occupied.candidate_hash, occupied.candidate_descriptor))
		} else {
			None
		})
		.collect())
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

	rx.await.map_err(|e| Error::QueryAvailabilityResponseChannel(e))
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
			chunk: erasure_chunk,
			tx,
		}
	)).await;

	rx.await.map_err(|e| Error::StoreChunkResponseChannel(e))
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

	while let Some((ancestor, ancestor_parent)) = iter.next().and_then(|a| iter.peek().map(|ap| (a, ap))) {
		if query_session_index_for_child(ctx, *ancestor_parent).await? != desired_session {
			break;
		}
		acc.push(ancestor);
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
