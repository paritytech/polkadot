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
//! which are intereseted in the relevant candidates.
//! Gossip messages received from other peers are verified and gossiped to interested
//! peers. Verified in this context means, the erasure chunks contained merkle proof
//! is checked.

use codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt};

use keystore::KeyStorePtr;
use sc_keystore as keystore;

use node_primitives::{ProtocolId, View};

use log::{trace, warn};
use polkadot_erasure_coding::{branch_hash, branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_primitives::v1::{
	AvailableData, BlakeTwo256, CommittedCandidateReceipt, CoreState, ErasureChunk,
	GlobalValidationData, Hash as Hash, HashT, Id as ParaId, LocalValidationData,
	OccupiedCoreAssumption, OmittedValidationData, PoV, ValidatorId, ValidatorIndex, ValidatorPair,
};
use polkadot_subsystem::messages::*;
use polkadot_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem,
	SubsystemContext, SubsystemError,
};
use sc_network::ReputationChange as Rep;
use sp_staking::SessionIndex;
use std::collections::{HashMap, HashSet};
use std::io;
use std::iter;

const TARGET: &'static str = "avad";

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Erasure(polkadot_erasure_coding::Error),
	#[from]
	Io(io::Error),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Subsystem(SubsystemError),
	#[from]
	RuntimeApi(RuntimeApiError),
	#[from]
	ChainApi(ChainApiError),

	Logic,
}

type Result<T> = std::result::Result<T, Error>;

const COST_MERKLE_PROOF_INVALID: Rep = Rep::new(-100, "Merkle proof was invalid");
const COST_NOT_A_LIVE_CANDIDATE: Rep = Rep::new(-51, "Candidate is not live");
const COST_MESSAGE_NOT_DECODABLE: Rep = Rep::new(-100, "Message is not decodable");
const COST_PEER_DUPLICATE_MESSAGE: Rep = Rep::new(-500, "Peer sent identical messages");
const BENEFIT_VALID_MESSAGE_FIRST: Rep = Rep::new(15, "Valid message with new information");
const BENEFIT_VALID_MESSAGE: Rep = Rep::new(10, "Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AvailabilityGossipMessage {
	/// Ankor hash of the candidate this is associated to.
	pub candidate_hash: Hash,
	/// The actual signed availability bitfield.
	pub erasure_chunk: ErasureChunk,
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
	/// Track all active peers and their views
	/// to determine what is relevant to them.
	peer_views: HashMap<PeerId, View>,

	/// Our own view.
	view: View,

	/// Caches a mapping of relay parents or ancestor to live candidate receipts.
	/// Allows fast intersection of live candidates with views and consecutive unioning.
	/// Maps relay parent / ancestor -> live candidate receipts + its hash.
	receipts: HashMap<Hash, HashSet<(Hash, CommittedCandidateReceipt)>>,

	/// Allow reverse caching of view checks.
	/// Maps candidate hash -> relay parent for extracting meta information from `PerRelayParent`.
	/// Note that the presence of this is not sufficient to determine if deletion is ok, i.e.
	/// two histories could cover this.
	reverse: HashMap<Hash, Hash>,

	/// Keeps track of which candidate receipts are required due to ancestors of which relay parents
	/// of our view.
	/// Maps ancestor -> relay parents in view
	ancestry: HashMap<Hash, HashSet<Hash>>,

	/// Track things needed to start and stop work on a particular relay parent.
	per_relay_parent: HashMap<Hash, PerRelayParent>,

	/// Track data that is specific to a candidate.
	per_candidate: HashMap<Hash, PerCandidate>,
}

#[derive(Debug, Clone, Default)]
struct PerCandidate {
	/// A Candidate and a set of known erasure chunks in form of messages to be gossiped / distributed if the peer view wants that.
	/// This is _across_ peers and not specific to a particular one.
	/// candidate hash + erasure chunk index -> gossip message
	message_vault: HashMap<u32, AvailabilityGossipMessage>,

	/// Track received candidate hashes and chunk indices from peers.
	received_messages: HashMap<PeerId, HashSet<(Hash, ValidatorIndex)>>,

	/// Track already sent candidate hashes and the erasure chunk index to the peers.
	sent_messages: HashMap<PeerId, HashSet<(Hash, ValidatorIndex)>>,
}

#[derive(Debug, Clone, Default)]
struct PerRelayParent {
	/// Set of `K` ancestors for this relay parent.
	ancestors: Vec<Hash>,

	/// The set of validators.
	validators: Vec<ValidatorId>,

	/// If this node is a validator, note the index in the validator set.
	validator_index: Option<ValidatorIndex>,
}

impl ProtocolState {
	// collects the relay_parents plus their ancestors
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
					.chain(iter::once(relay_parent.clone()))
			})
			.flatten()
			.collect::<HashSet<Hash>>()
	}

	/// Unionize all cached entries for the given relay parents and it's ancestors ancestors.
	/// Ignores all non existant relay parents, so this can be used directly with a peers view.
	/// Returns a map from candidate hash -> receipt
	fn cached_live_candidates_unioned<'a>(
		&'a self,
		relay_parents: impl IntoIterator<Item = &'a Hash> + 'a,
	) -> HashMap<Hash, CommittedCandidateReceipt> {
		let relay_parents_and_ancestors = self.extend_with_ancestors(relay_parents);
		relay_parents_and_ancestors
			.into_iter()
			.filter_map(|relay_parent_or_ancestor| self.receipts.get(&relay_parent_or_ancestor))
			.map(|receipt_set| receipt_set.into_iter())
			.flatten()
			.map(|(receipt_hash, receipt)| (receipt_hash.clone(), receipt.clone()))
			.collect::<HashMap<Hash, CommittedCandidateReceipt>>()
	}

	async fn add_relay_parent<Context>(
		&mut self,
		ctx: &mut Context,
		relay_parent: Hash,
	) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		let candidates =
			query_live_candidates(ctx, self, std::iter::once(relay_parent.clone())).await?;

		// collect the ancestors again from the hash map
		let ancestors = candidates
			.iter()
			.filter_map(|(ancestor_or_relay_parent, _receipt)| {
				if ancestor_or_relay_parent == &relay_parent {
					None
				} else {
					Some(ancestor_or_relay_parent.clone())
				}
			})
			.collect::<Vec<Hash>>();

		// mark all the ancestors as "needed" by this newly added relay parent
		for ancestor in ancestors.iter() {
			self.ancestry
				.entry(ancestor.clone())
				.or_default()
				.insert(relay_parent.clone());
		}

		let per_relay_parent = self
			.per_relay_parent
			.get_mut(&relay_parent)
			.expect("Relay parent is initialized on overseer signal. qed");

		per_relay_parent.ancestors = ancestors;

		Ok(())
	}

	fn remove_relay_parent(&mut self, relay_parent: &Hash) -> Result<()> {
		// we might be ancestor of some other relay_parent
		if let Some(ref mut descendants) = self.ancestry.get_mut(relay_parent) {
			// if we were the last user, and it is
			// not explicitly set to be worked on by the overseer
			if descendants.is_empty() {
				// remove from the ancestry index
				self.ancestry.remove(relay_parent);
				// and also remove the actual receipt
				self.receipts.remove(relay_parent);
				self.per_candidate.remove(relay_parent);
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
						self.receipts.remove(&ancestor);
						self.per_candidate.remove(&ancestor);
					}
				}
			}
		}
		Ok(())
	}
}

fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
	AllMessages::AvailabilityDistribution(AvailabilityDistributionMessage::NetworkBridgeUpdate(n))
}

/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	bridge_message: NetworkBridgeEvent,
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
			handle_peer_view_change(ctx, state, peerid, view).await?;
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			handle_our_view_change(ctx, state, view).await?;
		}
		NetworkBridgeEvent::PeerMessage(remote, bytes) => {
			if let Ok(gossiped_availability) =
				AvailabilityGossipMessage::decode(&mut (bytes.as_slice()))
			{
				trace!(
					target: TARGET,
					"Received availability gossip from peer {:?}",
					&remote
				);
				process_incoming_peer_message(ctx, state, remote, gossiped_availability).await?;
			} else {
				modify_reputation(ctx, remote, COST_MESSAGE_NOT_DECODABLE).await?;
			}
		}
	}
	Ok(())
}

fn derive_erasure_chunks_with_proofs(
	n_validators: usize,
	validator_index: ValidatorIndex,
	available_data: &AvailableData,
) -> Result<Vec<ErasureChunk>> {
	if n_validators <= validator_index as usize {
		warn!("Validator index is outside the validator set range");
		return Err(Error::Logic);
	}
	let chunks: Vec<Vec<u8>> = if let Ok(chunks) = obtain_chunks(n_validators, available_data) {
		chunks
	} else {
		warn!("Failed to create erasure chunks");
		return Err(Error::Logic);
	};

	// create proofs for each erasure chunk
	let branches = branches(chunks.as_ref());

	let erasure_chunks = branches
		.map(|(proof, chunk)| ErasureChunk {
			chunk: chunk.to_vec(),
			index: validator_index,
			proof,
		})
		.collect::<Vec<ErasureChunk>>();

	Ok(erasure_chunks)
}

/// Handle the changes necassary when our view changes.
async fn handle_our_view_change<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	view: View,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let old_view = std::mem::replace(&mut (state.view), view);

	// needed due to borrow rules
	let view = state.view.clone();
	let added = view.difference(&old_view).collect::<Vec<&'_ Hash>>();

	// add all the relay parents and fill the cache
	for added in added.clone() {
		state.add_relay_parent(ctx, added.clone()).await?;
	}

	// handle all candidates
	for (candidate_hash, candidate_receipt) in state.cached_live_candidates_unioned(added) {
		let added = candidate_receipt.descriptor().relay_parent;
		let desc = candidate_receipt.descriptor();
		let para = desc.para_id;

		let per_relay_parent =
			if let Some(per_relay_parent) = state.per_relay_parent.get_mut(&desc.relay_parent) {
				per_relay_parent
			} else {
				// could only happen if the candidate is related
				// to an ancestor which is not a relay parent itself
				continue;
			};

		// assure the node has the validator role
		let validator_index = if let Some(validator_index) = per_relay_parent.validator_index {
			validator_index
		} else {
			continue;
		};

		// pull the proof of validity
		let available_data = if let Some(available_data) =
			query_available_data(ctx, candidate_hash.clone()).await?
		{
			available_data
		} else {
			continue;
		};

		let erasure_chunks = if let Ok(erasure_chunks) = derive_erasure_chunks_with_proofs(
			per_relay_parent.validators.len(),
			validator_index,
			&available_data,
		) {
			erasure_chunks
		} else {
			return Ok(());
		};

		if let Some(erasure_chunk) = erasure_chunks.get(validator_index as usize) {
			match store_chunk(ctx, candidate_hash, validator_index, erasure_chunk.clone()).await {
				Err(_e) => warn!(target: TARGET, "Failed to send store message to overseer"),
				Ok(Err(())) => warn!(target: TARGET, "Failed to store our own erasure chunk"),
				Ok(Ok(())) => {}
			}
		} else {
			warn!(
				target: TARGET,
				"Our validation index is out of bounds, no associated message"
			);
			return Ok(());
		}

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
		for erasure_chunk in erasure_chunks {
			// only the peers which did not receive this particular erasure chunk
			let per_candidate = state
				.per_candidate
				.entry(candidate_hash.clone())
				.or_default();
			let peers = peers
				.iter()
				.filter(|peer| {
					!per_candidate
						.sent_messages
						.get(*peer)
						.filter(|set| {
							// peer already received this message
							set.contains(&(candidate_hash.clone(), erasure_chunk.index))
						})
						.is_some()
				})
				.map(|peer| peer.clone())
				.collect::<Vec<_>>();
			let message = AvailabilityGossipMessage {
				candidate_hash,
				erasure_chunk,
			};

			// let per_candidate = state.per_candidate.entry(candidate_hash.clone()).or_default();
			send_tracked_gossip_message_to_peers(ctx, per_candidate, peers, message).await?;
		}
	}

	// cleanup the removed relay parents and their states
	let removed = old_view.difference(&view).collect::<Vec<_>>();
	for removed in removed {
		state.remove_relay_parent(&removed)?;
	}
	Ok(())
}

#[inline(always)]
async fn send_tracked_gossip_message_to_peers<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	peers: Vec<PeerId>,
	message: AvailabilityGossipMessage,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	send_tracked_gossip_messages_to_peers(ctx, per_candidate, peers, iter::once(message)).await
}

#[inline(always)]
async fn send_tracked_gossip_messages_to_peer<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	peer: PeerId,
	message_iter: impl IntoIterator<Item = AvailabilityGossipMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	send_tracked_gossip_messages_to_peers(ctx, per_candidate, vec![peer], message_iter).await
}

async fn send_tracked_gossip_messages_to_peers<Context>(
	ctx: &mut Context,
	per_candidate: &mut PerCandidate,
	peers: Vec<PeerId>,
	message_iter: impl IntoIterator<Item = AvailabilityGossipMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	for message in message_iter {
		for peer in peers.clone() {
			let message_id = (message.candidate_hash, message.erasure_chunk.index);
			per_candidate
				.sent_messages
				.entry(peer)
				.or_default()
				.insert(message_id);
		}

		let encoded = message.encode();
		per_candidate
			.message_vault
			.insert(message.erasure_chunk.index, message);

		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendMessage(
				peers.clone(),
				AvailabilityDistributionSubsystem::PROTOCOL_ID,
				encoded,
			),
		))
		.await
		.map_err::<Error, _>(Into::into)?;
	}

	Ok(())
}

// Send the difference between two views which were not sent
// to that particular peer.
async fn handle_peer_view_change<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	view: View,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let current = state.peer_views.entry(origin.clone()).or_default();

	let delta_vec: Vec<Hash> = (*current).difference(&view).cloned().collect();

	*current = view;

	// only contains the intersection of what we are interested and
	// the union of all relay parent's candidates.
	let delta_candidates = state.cached_live_candidates_unioned(delta_vec.iter());

	// Send all messages we've seen before and the peer is now interested
	// in to that peer.

	for (candidate_hash, receipt) in delta_candidates {
		let per_candidate = state.per_candidate.entry(candidate_hash).or_default();

		let messages = if let Some(per_relay_parent) = state
			.per_relay_parent
			.get_mut(&receipt.descriptor().relay_parent)
		{
			// obtain the relevant chunk indices not sent yet
			let messages = ((0 as ValidatorIndex)
				..(per_relay_parent.validators.len() as ValidatorIndex))
				.into_iter()
				.filter(|erasure_chunk_index: &ValidatorIndex| {
					// check if that erasure chunk was already sent before
					if let Some(sent_set) = per_candidate.sent_messages.get(&origin) {
						!sent_set.contains(&(candidate_hash, *erasure_chunk_index))
					} else {
						true
					}
				})
				.filter_map(|erasure_chunk_index: ValidatorIndex| {
					// try to pick up the message from the message vault
					per_candidate.message_vault.get(&erasure_chunk_index)
				})
				.cloned()
				.collect::<HashSet<_>>();
			messages
		} else {
			continue;
		};

		send_tracked_gossip_messages_to_peer(ctx, per_candidate, origin.clone(), messages).await?;
	}
	Ok(())
}

/// Obtain the first key with a signing key, which must be ours. We obtain the index as `ValidatorIndex`
/// If we cannot find a key in the validator set, which we could use.
fn obtain_our_validator_index<Context>(
	validators: &[ValidatorId],
	keystore: KeyStorePtr,
) -> Option<ValidatorIndex> {
	let keystore = keystore.read();
	validators.iter().enumerate().find_map(|(idx, v)| {
		keystore
			.key_pair::<ValidatorPair>(&v)
			.ok()
			.map(move |_| idx as ValidatorIndex)
	})
}

/// Handle an incoming message from a peer.
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	origin: PeerId,
	message: AvailabilityGossipMessage,
) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	// obtain the set of candidates we are interested in based on our current view
	let live_candidates = state.cached_live_candidates_unioned(state.view.0.iter());

	// check if the candidate is of interest
	let live_candidate = if let Some(live_candidate) = live_candidates.get(&message.candidate_hash)
	{
		live_candidate
	} else {
		return modify_reputation(ctx, origin, COST_NOT_A_LIVE_CANDIDATE).await;
	};

	// check the merkle proof
	let root = &live_candidate.commitments().erasure_root;
	let anticipated_hash = if let Ok(hash) = branch_hash(
		root,
		&message.erasure_chunk.proof,
		message.erasure_chunk.index as usize,
	) {
		hash
	} else {
		return modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
	};

	let erasure_chunk_hash = BlakeTwo256::hash(&message.erasure_chunk.chunk);
	if anticipated_hash != erasure_chunk_hash {
		return modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
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
				return modify_reputation(ctx, origin, COST_PEER_DUPLICATE_MESSAGE).await;
			} else {
				received_set.insert(message_id.clone());
			}
		}

		// insert into known messages and change reputation
		if per_candidate
			.message_vault
			.insert(message_id.1, message.clone())
			.is_none()
		{
			modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE).await?;
		} else {
			modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE_FIRST).await?;
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
		.filter(|peer| {
			let peer: PeerId = peer.clone();
			// avoid sending duplicate messages
			per_candidate
				.sent_messages
				.entry(peer)
				.or_default()
				.contains(&message_id)
		})
		.collect::<Vec<_>>();

	// gossip that message to interested peers
	send_tracked_gossip_message_to_peers(ctx, per_candidate, peers, message).await
}

/// The bitfield distribution subsystem.
pub struct AvailabilityDistributionSubsystem {
	/// Pointer to a keystore, which is required for determining this nodes validator index.
	keystore: KeyStorePtr,
}

impl AvailabilityDistributionSubsystem {
	/// The protocol identifier for bitfield distribution.
	const PROTOCOL_ID: ProtocolId = *b"avad";

	/// Number of ancestors to keep around for the relay-chain heads.
	const K: usize = 3;

	/// Create a new instance of the availability distribution.
	fn new(keystore: KeyStorePtr) -> Self {
		Self { keystore }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, mut ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
	{
		// startup: register the network protocol with the bridge.
		ctx.send_message(AllMessages::NetworkBridge(
			NetworkBridgeMessage::RegisterEventProducer(Self::PROTOCOL_ID, network_update_message),
		))
		.await
		.map_err::<Error, _>(Into::into)?;

		// work: process incoming messages from the overseer.
		let mut state = ProtocolState::default();
		loop {
			let message = ctx.recv().await.map_err::<Error, _>(Into::into)?;
			match message {
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::NetworkBridgeUpdate(event),
				} => {
					trace!(target: TARGET, "Processing NetworkMessage");
					// a network message was received
					if let Err(e) = handle_network_msg(&mut ctx, &mut state, event).await {
						warn!(
							target: TARGET,
							"Failed to handle incomming network messages: {:?}", e
						);
					}
				}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated,
					deactivated,
				})) => {
					for relay_parent in activated {
						trace!(target: TARGET, "Start {:?}", relay_parent);
						let per_relay_parent = state
							.per_relay_parent
							.entry(relay_parent.clone())
							.or_default();
						let validators = query_validators(&mut ctx, relay_parent).await?;
						per_relay_parent.validator_index = obtain_our_validator_index::<Context>(
							&validators,
							self.keystore.clone(),
						);
						per_relay_parent.validators = validators;
					}
					for relay_parent in deactivated {
						trace!(target: TARGET, "Stop {:?}", relay_parent);
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					trace!(target: TARGET, "Conclude");
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
		SpawnedSubsystem {
			name: "availability-distribution",
			future: Box::pin(async move { self.run(ctx) }.map(|_| ())),
		}
	}
}

/// Obtain all live candidates based on an iterator of relay heads.
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
		let paras = query_para_ids(ctx, relay_parent.clone()).await?;
		for para in paras {
			if let Some(ccr) = query_pending_availability(ctx, relay_parent, para).await? {
				live_candidates.insert(ccr);
			}
		}
	}
	Ok(live_candidates)
}

/// Obtain all live canddidates based on an iterator or relay heads including `k` ancestors.
///
/// Relay parent.
async fn query_live_candidates<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	relay_parents: impl IntoIterator<Item = Hash>,
) -> Result<HashMap<Hash, (Hash, CommittedCandidateReceipt)>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let iter = relay_parents.into_iter();
	let hint = iter.size_hint();

	let capacity = hint.1.unwrap_or(hint.0) * (1 + AvailabilityDistributionSubsystem::K);
	let mut live_candidates =
		HashMap::<Hash, (Hash, CommittedCandidateReceipt)>::with_capacity(capacity);

	for relay_parent in iter {
		// direct candidates for one relay parent
		let receipts =
			query_live_candidates_without_ancestors(ctx, iter::once(relay_parent.clone())).await?;
		live_candidates.extend(
			receipts
				.into_iter()
				.map(|receipt| (relay_parent.clone(), (receipt.hash(), receipt))),
		);

		// register one of relay parents (not the ancestors)
		let ancestors = query_up_to_k_ancestors_in_same_session(
			ctx,
			relay_parent,
			AvailabilityDistributionSubsystem::K,
		)
		.await?;

		// ancestors might overlap, so check the cache too
		let unknown = ancestors
			.into_iter()
			.filter(|relay_parent| {
				// use the ones which we pulled before
				// but keep the unknown relay parents
				state
					.receipts
					.get(relay_parent)
					.and_then(|receipts| {
						// directly extend the live_candidates with the cached value
						live_candidates.extend(receipts.into_iter().map(
							|(receipt_hash, receipt)| {
								(
									relay_parent.clone(),
									(receipt_hash.clone(), receipt.clone()),
								)
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

	// register the relation of relay_parent to candidate..
	// ..and the reverse association.
	for (relay_parent_or_ancestor, (receipt_hash, receipt)) in live_candidates.clone() {
		state
			.reverse
			.insert(receipt_hash.clone(), relay_parent_or_ancestor.clone());
		state.per_candidate.entry(receipt_hash.clone()).or_default();
		state
			.receipts
			.entry(relay_parent_or_ancestor)
			.or_default()
			.insert((receipt_hash, receipt));
	}

	Ok(live_candidates)
}

/// Query all para IDs.
async fn query_para_ids<Context>(ctx: &mut Context, relay_parent: Hash) -> Result<Vec<ParaId>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	)))
	.await
	.map_err::<Error, _>(Into::into)?;

	let all_para_ids: Vec<_> = rx
		.await
		.map_err::<Error, _>(Into::into)?
		.map_err::<Error, _>(Into::into)?;

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

/// Modify the reputation of a peer based on its behaviour.
async fn modify_reputation<Context>(ctx: &mut Context, peer: PeerId, rep: Rep) -> Result<()>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	trace!(
		target: TARGET,
		"Reputation change of {:?} for peer {:?}",
		rep,
		peer
	);
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	))
	.await
	.map_err::<Error, _>(Into::into)
}

/// Query the proof of validity for a particular candidate hash.
async fn query_available_data<Context>(
	ctx: &mut Context,
	candidate_hash: Hash,
) -> Result<Option<AvailableData>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx),
	))
	.await
	.map_err::<Error, _>(Into::into)?;
	rx.await.map_err::<Error, _>(Into::into)
}

async fn store_chunk<Context>(
	ctx: &mut Context,
	candidate_hash: Hash,
	validator_index: ValidatorIndex,
	erasure_chunk: ErasureChunk,
) -> Result<std::result::Result<(), ()>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::StoreChunk(candidate_hash, validator_index, erasure_chunk, tx),
	))
	.await
	.map_err::<Error, _>(Into::into)?;
	rx.await.map_err::<Error, _>(Into::into)
}

/// Request the head data for a particular para.
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
	)))
	.await
	.map_err::<Error, _>(Into::into)?;
	rx.await
		.map_err::<Error, _>(Into::into)?
		.map_err::<Error, _>(Into::into)
}

/// Query the validator set.
async fn query_validators<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<Vec<ValidatorId>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent.clone(),
		RuntimeApiRequest::Validators(tx),
	));

	ctx.send_message(query_validators)
		.await
		.map_err::<Error, _>(Into::into)?;
	rx.await
		.map_err::<Error, _>(Into::into)?
		.map_err::<Error, _>(Into::into)
}

/// Query the hash of the `K` ancestors
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
		.await
		.map_err::<Error, _>(Into::into)?;
	rx.await
		.map_err::<Error, _>(Into::into)?
		.map_err::<Error, _>(Into::into)
}

/// Query the session index of a relay parent
async fn query_session_index_for_child<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<SessionIndex>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_session_idx_for_child = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent.clone(),
		RuntimeApiRequest::SessionIndexForChild(tx),
	));

	ctx.send_message(query_session_idx_for_child)
		.await
		.map_err::<Error, _>(Into::into)?;
	rx.await
		.map_err::<Error, _>(Into::into)?
		.map_err::<Error, _>(Into::into)
}

/// Queries up to k ancestors with the constraints of equiv session
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
	let mut acc = Vec::with_capacity(ancestors.len() - 1);

	// iterate from oldest to youngest
	let mut iter = ancestors.into_iter().rev();
	if let Some(oldest) = iter.next() {
		let mut child_session_index = query_session_index_for_child(ctx, oldest).await?;
		for ancestor in iter {
			let ancestor_session_index = child_session_index;
			child_session_index = query_session_index_for_child(ctx, ancestor).await?;

			if desired_session == ancestor_session_index {
				acc.push(ancestor);
			}
		}
	}
	debug_assert!(acc.len() <= k);
	Ok(acc)
}

#[cfg(test)]
mod test {
	use super::*;
	use assert_matches::assert_matches;
	use bitvec::bitvec;
	use maplit::hashmap;
	use polkadot_primitives::v1::{
		AvailableData, BlockData, CandidateCommitments, CandidateDescriptor, GlobalValidationData,
		GroupIndex, GroupRotationInfo, HeadData, LocalValidationData, OccupiedCore,
		OmittedValidationData, PoV, ScheduledCore, ValidatorPair,
	};
	use polkadot_subsystem::test_helpers;

	use futures::{channel::oneshot, executor, future, Future};
	use smallvec::smallvec;
	use smol_timeout::TimeoutExt;
	use sp_core::crypto::Pair;
	use std::time::Duration;

	macro_rules! view {
		( $( $hash:expr ),* $(,)? ) => [
			View(vec![ $( $hash.clone() ),* ])
		];
	}

	struct TestHarness {
		virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	}

	fn test_harness<T: Future<Output = ()>>(
		keystore: KeyStorePtr,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				Some("polkadot_availability_distribution"),
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();
		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = AvailabilityDistributionSubsystem::new(keystore);
		let subsystem = subsystem.run(context);

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(100_000);

	async fn overseer_signal(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
		signal: OverseerSignal,
	) {
		overseer
			.send(FromOverseer::Signal(signal))
			.timeout(TIMEOUT)
			.await
			.expect("10ms is more than enough for sending signals.");
	}

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
		msg: AvailabilityDistributionMessage,
	) {
		log::trace!("Sending message:\n{:?}", &msg);
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect("10ms is more than enough for sending messages.");
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	) -> AllMessages {
		log::trace!("Waiting for message ...");
		let msg = overseer
			.recv()
			.timeout(TIMEOUT)
			.await
			.expect("TIMEOUT is enough to recv.");
		log::trace!("Received message:\n{:?}", &msg);
		msg
	}

	fn dummy_occupied_core(para: ParaId) -> CoreState {
		CoreState::Occupied(OccupiedCore {
			para_id: para,
			next_up_on_available: None,
			occupied_since: 0,
			time_out_at: 5,
			next_up_on_time_out: None,
			availability: Default::default(),
			group_responsible: GroupIndex::from(0),
		})
	}

	use sp_keyring::Sr25519Keyring;

	struct TestState {
		chain_ids: Vec<ParaId>,
		validators: Vec<Sr25519Keyring>,
		validator_public: Vec<ValidatorId>,
		validator_index: Option<ValidatorIndex>,
		validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
		head_data: HashMap<ParaId, HeadData>,
		keystore: KeyStorePtr,
		relay_parent: Hash,
		ancestors: Vec<Hash>,
		availability_cores: Vec<CoreState>,
		global_validation_data: GlobalValidationData,
		local_validation_data: LocalValidationData,
	}

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	impl Default for TestState {
		fn default() -> Self {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);

			let chain_ids = vec![chain_a, chain_b];

			let validators = vec![
				Sr25519Keyring::Ferdie, // <- this node, role: validator
				Sr25519Keyring::Alice,
				Sr25519Keyring::Bob,
				Sr25519Keyring::Charlie,
				Sr25519Keyring::Dave,
			];

			let keystore = keystore::Store::new_in_memory();

			keystore
				.write()
				.insert_ephemeral_from_seed::<ValidatorPair>(&validators[0].to_seed())
				.expect("Insert key into keystore");

			let validator_public = validator_pubkeys(&validators);

			let validator_groups = vec![vec![2, 0, 4], vec![1], vec![3]];
			let group_rotation_info = GroupRotationInfo {
				session_start_block: 0,
				group_rotation_frequency: 100,
				now: 1,
			};
			let validator_groups = (validator_groups, group_rotation_info);

			let availability_cores = vec![
				CoreState::Scheduled(ScheduledCore {
					para_id: chain_ids[0],
					collator: None,
				}),
				CoreState::Scheduled(ScheduledCore {
					para_id: chain_ids[1],
					collator: None,
				}),
			];

			let mut head_data = HashMap::new();
			head_data.insert(chain_a, HeadData(vec![4, 5, 6]));

			let ancestors = vec![
				Hash::repeat_byte(0x44),
				Hash::repeat_byte(0x33),
				Hash::repeat_byte(0x22),
			];
			let relay_parent = Hash::repeat_byte(0x05);

			let local_validation_data = LocalValidationData {
				parent_head: HeadData(vec![7, 8, 9]),
				balance: Default::default(),
				code_upgrade_allowed: None,
				validation_code_hash: Default::default(),
			};

			let global_validation_data = GlobalValidationData {
				max_code_size: 1000,
				max_head_data_size: 1000,
				block_number: Default::default(),
			};

			let validator_index = Some((validators.len() - 1) as ValidatorIndex);

			Self {
				chain_ids,
				keystore,
				validators,
				validator_public,
				validator_groups,
				availability_cores,
				head_data,
				local_validation_data,
				global_validation_data,
				relay_parent,
				ancestors,
				validator_index,
			}
		}
	}

	fn make_erasure_root(test: &TestState, pov: PoV) -> Hash {
		let omitted_validation = OmittedValidationData {
			global_validation: test.global_validation_data.clone(),
			local_validation: test.local_validation_data.clone(),
		};

		let available_data = AvailableData {
			omitted_validation,
			pov,
		};

		let chunks = obtain_chunks(test.validators.len(), &available_data).unwrap();
		branches(&chunks).root()
	}

	#[derive(Default)]
	struct TestCandidateBuilder {
		para_id: ParaId,
		head_data: HeadData,
		pov_hash: Hash,
		relay_parent: Hash,
		erasure_root: Hash,
	}

	impl TestCandidateBuilder {
		fn build(self) -> CommittedCandidateReceipt {
			CommittedCandidateReceipt {
				descriptor: CandidateDescriptor {
					para_id: self.para_id,
					pov_hash: self.pov_hash,
					relay_parent: self.relay_parent,
					..Default::default()
				},
				commitments: CandidateCommitments {
					head_data: self.head_data,
					erasure_root: self.erasure_root,
					..Default::default()
				},
			}
		}
	}

	fn valid_availability_gossip(
		candidate_hash: Hash,
		validator_count: usize,
		validator_index: ValidatorIndex,
		erasure_chunk_index: u32,
	) -> AvailabilityGossipMessage {
		let available_data = AvailableData {
			pov: PoV {
				block_data: BlockData(vec![0x77; 453]),
			},
			omitted_validation: OmittedValidationData {
				global_validation: GlobalValidationData::default(),
				local_validation: LocalValidationData {
					validation_code_hash: candidate_hash.clone(),
					..Default::default()
				},
			},
		};
		let erasure_chunks =
			derive_erasure_chunks_with_proofs(validator_count, validator_index, &available_data)
				.expect("Generated must work");

		let erasure_chunk: ErasureChunk = erasure_chunks
			.get(erasure_chunk_index as usize)
			.expect("Must be valid or input is oob")
			.clone();

		AvailabilityGossipMessage {
			candidate_hash,
			erasure_chunk,
		}
	}

	#[test]
	fn reputation_verification() {
		let test_state = TestState::default();

		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

			let pov_block_a = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let pov_block_b = PoV {
				block_data: BlockData(vec![45, 46, 47]),
			};

			let pov_hash_a = pov_block_a.hash();
			let pov_hash_b = pov_block_b.hash();

			let candidate_a = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash: pov_hash_a,
				erasure_root: make_erasure_root(&test_state, pov_block_a.clone()),
				..Default::default()
			}.build();

			let candidate_b = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash: pov_hash_b,
				erasure_root: make_erasure_root(&test_state, pov_block_b.clone()),
				head_data: expected_head_data.clone(),
				..Default::default()
			}.build();

			let TestState {
				chain_ids,
				keystore,
				validators,
				validator_public,
				validator_groups,
				availability_cores,
				head_data,
				local_validation_data,
				global_validation_data,
				relay_parent,
				ancestors,
				validator_index,
			} = test_state;

			let validator_index: ValidatorIndex = validator_index.unwrap();

			let relay_parent_x = relay_parent;
			// y is an ancestor of x
			let relay_parent_y = ancestors[0];

			let peer_a = PeerId::random();
			let peer_b = PeerId::random();
			assert_ne!(&peer_a, &peer_b);

			log::trace!("peer A: {:?}", peer_a);
			log::trace!("peer B: {:?}", peer_b);

			log::trace!("candidate A: {:?}", candidate_a.hash());
			log::trace!("candidate B: {:?}", candidate_b.hash());

			overseer_signal(
				&mut virtual_overseer,
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: smallvec![relay_parent_x.clone()],
					deactivated: smallvec![],
				}),
			)
			.await;

			// ignore event producer registration
			let _ = overseer_recv(&mut virtual_overseer).await;

			// obtain the validators per relay parent
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::Validators(tx),
				)) => {
					assert_eq!(relay_parent, relay_parent_x);
					tx.send(Ok(validator_public.clone())).unwrap();
				}
			);

			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::OurViewChange(view![relay_parent_x,]),
				),
			)
			.await;

			// subsystem peer id collection
			// which will query the availability cores
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::AvailabilityCores(tx)
				)) => {
					assert_eq!(relay_parent, relay_parent_x);
					// respond with a set of availability core states
					tx.send(Ok(vec![
						dummy_occupied_core(chain_ids[0]),
						dummy_occupied_core(chain_ids[1])
					])).unwrap();
				}
			);

			// now each of the relay parents in the view (1) will
			// be queried for candidate pending availability
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidatePendingAvailability(para, tx)
				)) => {
					assert_eq!(relay_parent, relay_parent_x);
					assert_eq!(para, chain_ids[0]);
					tx.send(Ok(Some(CommittedCandidateReceipt {
						descriptor: CandidateDescriptor {
							para_id: para,
							relay_parent,
							.. Default::default()
						},
						.. Default::default()
					}))).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidatePendingAvailability(para, tx)
				)) => {
					assert_eq!(relay_parent, relay_parent_x);
					assert_eq!(para, chain_ids[1]);
					tx.send(Ok(Some(CommittedCandidateReceipt {
						descriptor: CandidateDescriptor {
							para_id: para,
							relay_parent,
							.. Default::default()
						},
						.. Default::default()
					}))).unwrap();
				}
			);

			// query of k ancestors, we only provide one
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: relay_parent,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(relay_parent, relay_parent_x);
					assert_eq!(k, AvailabilityDistributionSubsystem::K + 1);
					tx.send(Ok(vec![relay_parent_y.clone(), relay_parent_y.clone()])).unwrap();
				}
			);

			for _ in 0..3 {
				// state query for each of them
				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						relay_parent,
						RuntimeApiRequest::SessionIndexForChild(tx)
					)) => {
						tx.send(Ok(1 as SessionIndex)).unwrap();
					}
				);
			}

			// query para ids for that ancestor relay parent again
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::AvailabilityCores(tx),
				)) => {
					assert_eq!(relay_parent, relay_parent_y);
					tx.send(Ok(vec![
						CoreState::Occupied(OccupiedCore {
							para_id: chain_ids[0].clone(),
							next_up_on_available: None,
							occupied_since: 0,
							time_out_at: 10,
							next_up_on_time_out: None,
							availability: Default::default(),
							group_responsible: GroupIndex::from(0),
						}),
						CoreState::Free,
						CoreState::Free,
						CoreState::Occupied(OccupiedCore {
							para_id: chain_ids[1].clone(),
							next_up_on_available: None,
							occupied_since: 1,
							time_out_at: 7,
							next_up_on_time_out: None,
							availability: Default::default(),
							group_responsible: GroupIndex::from(0),
						}),
						CoreState::Free,
						CoreState::Free,
					])).unwrap();
				}
			);

			// query the availability cores for each of the paras (2)
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(
						relay_parent,
						RuntimeApiRequest::CandidatePendingAvailability(para, tx),
					)
				) => {
					assert_eq!(relay_parent, relay_parent_y);
					assert_eq!(para, chain_ids[0]);
					tx.send(Ok(Some(
						candidate_a.clone()
					))).unwrap();
				}
			);
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidatePendingAvailability(para, tx),
				)) => {
					assert_eq!(relay_parent, relay_parent_y);
					assert_eq!(para, chain_ids[1]);
					tx.send(Ok(Some(
						candidate_a.clone()
					))).unwrap();
				}
			);

			// query the available data incl PoV from the availability store

			for _ in 0usize..2 {
				// query the available data incl PoV from the availability store
				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::QueryAvailableData(
							candidate_hash,
							tx,
						)
					) => {
						// assert_eq!(candidate_hash, candidate_b.hash());
						tx.send(Some(
							AvailableData {
								pov: pov_block_a.clone(),
								omitted_validation: OmittedValidationData {
									local_validation: local_validation_data.clone(),
									global_validation: global_validation_data.clone(),
								},
							}
						)).unwrap();
					}
				);

				// query the available data incl PoV from the availability store
				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::StoreChunk(
							candidate_hash,
							idx,
							chunk,
							tx,
						)
					) => {
						tx.send(
							Ok(())
						).unwrap();
					}
				);

				for _ in 0usize..5 {
					assert_matches!(
						overseer_recv(&mut virtual_overseer).await,
						AllMessages::NetworkBridge(_) => {}
					);
				}

			}

			// setup peer a with interest in parent x
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(peer_a.clone(), ObservedRole::Full),
				),
			)
			.await;

			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![relay_parent_x]),
				),
			)
			.await;
			// setup peer b with interest in parent y
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full),
				),
			)
			.await;

			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![relay_parent_y]),
				),
			)
			.await;

			/////////////////////////////////////////////////////////
			// ready for action

			// check if garbage messages are detected and peer rep is changed as expected
			let garbage = b"I am garbage";

			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						// AvailabilityDistributionSubsystem::PROTOCOL_ID,
						garbage.to_vec(),
					),
				),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_MESSAGE_NOT_DECODABLE);
				}
			);

			let valid: AvailabilityGossipMessage =
				valid_availability_gossip(candidate_a.hash(), validators.len(), validator_index, 2);

			// valid (first, from b)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(peer_b.clone(), valid.encode()),
				),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST);
				}
			);

			// valid (duplicate, from b)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(peer_b.clone(), valid.encode()),
				),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE);
				}
			);

			// valid (second, from a)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(peer_a.clone(), valid.encode()),
				),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE);
				}
			);
		});
	}
}
