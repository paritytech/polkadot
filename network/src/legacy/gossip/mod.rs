// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Gossip messages and the message validator.
//!
//! At the moment, this module houses 2 gossip protocols central to Polkadot.
//!
//! The first is the attestation-gossip system, which aims to circulate parachain
//! candidate attestations by validators at leaves of the block-DAG.
//!
//! The second is the inter-chain message queue routing gossip, which aims to
//! circulate message queues between parachains, which remain un-routed as of
//! recent leaves.
//!
//! These gossip systems do not have any form of sybil-resistance in terms
//! of the nodes which can participate. It could be imposed e.g. by limiting only to
//! validators, but this would prevent message queues from getting into the hands
//! of collators and of attestations from getting into the hands of fishermen.
//! As such, we take certain precautions which allow arbitrary full nodes to
//! join the gossip graph, as well as validators (who are likely to be well-connected
//! amongst themselves).
//!
//! The first is the notion of a neighbor packet. This is a packet sent between
//! neighbors of the gossip graph to inform each other of their current protocol
//! state. As of this writing, for both attestation and message-routing gossip,
//! the only necessary information here is a (length-limited) set of perceived
//! leaves of the block-DAG.
//!
//! These leaves can be used to derive what information a node is willing to accept
//! There is typically an unbounded amount of possible "future" information relative to
//! any protocol state. For example, attestations or unrouted message queues from millions
//! of blocks after a known protocol state. The neighbor packet is meant to avoid being
//! spammed by illegitimate future information, while informing neighbors of when
//! previously-future and now current gossip messages would be accepted.
//!
//! Peers who send information which was not allowed under a recent neighbor packet
//! will be noted as non-beneficial to Substrate's peer-set management utility.

use sp_runtime::traits::{BlakeTwo256, Hash as HashT};
use sp_blockchain::Error as ClientError;
use sc_network::{ObservedRole, PeerId, ReputationChange};
use sc_network::NetworkService;
use sc_network_gossip::{
	ValidationResult as GossipValidationResult,
	ValidatorContext, MessageIntent,
};
use polkadot_validation::{SignedStatement};
use polkadot_primitives::v0::{
	Block, Hash,
	ParachainHost, ValidatorId, ErasureChunk as PrimitiveChunk, SigningContext, PoVBlock,
};
use polkadot_erasure_coding::{self as erasure};
use codec::{Decode, Encode};
use sp_api::ProvideRuntimeApi;

use std::collections::HashMap;
use std::sync::Arc;

use arrayvec::ArrayVec;
use futures::prelude::*;
use parking_lot::{Mutex, RwLock};

use crate::legacy::{GossipMessageStream, GossipService};

use attestation::{View as AttestationView, PeerData as AttestationPeerData};

mod attestation;

/// The engine ID of the polkadot attestation system.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot1";
pub const POLKADOT_PROTOCOL_NAME: &[u8] = b"/polkadot/legacy/1";

// arbitrary; in practice this should not be more than 2.
pub(crate) const MAX_CHAIN_HEADS: usize = 5;

/// Type alias for a bounded vector of leaves.
pub type LeavesVec = ArrayVec<[Hash; MAX_CHAIN_HEADS]>;

mod benefit {
	use sc_network::ReputationChange as Rep;
	/// When a peer sends us a previously-unknown candidate statement.
	pub const NEW_CANDIDATE: Rep = Rep::new(100, "Polkadot: New candidate");
	/// When a peer sends us a previously-unknown attestation.
	pub const NEW_ATTESTATION: Rep = Rep::new(50, "Polkadot: New attestation");
	/// When a peer sends us a previously-unknown pov-block
	pub const NEW_POV_BLOCK: Rep = Rep::new(150, "Polkadot: New PoV block");
	/// When a peer sends us a previously-unknown erasure chunk.
	pub const NEW_ERASURE_CHUNK: Rep = Rep::new(10, "Polkadot: New erasure chunk");
}

mod cost {
	use sc_network::ReputationChange as Rep;
	/// No cost. This will not be reported.
	pub const NONE: Rep = Rep::new(0, "");
	/// A peer sent us an attestation and we don't know the candidate.
	pub const ATTESTATION_NO_CANDIDATE: Rep = Rep::new(-100, "Polkadot: No candidate");
	/// A peer sent us a pov-block and we don't know the candidate or the leaf.
	pub const POV_BLOCK_UNWANTED: Rep = Rep::new(-500, "Polkadot: No candidate");
	/// A peer sent us a pov-block message with wrong data.
	pub const POV_BLOCK_BAD_DATA: Rep = Rep::new(-1000, "Polkadot: Bad PoV-block data");
	/// A peer sent us a statement we consider in the future.
	pub const FUTURE_MESSAGE: Rep = Rep::new(-100, "Polkadot: Future message");
	/// A peer sent us a statement from the past.
	pub const PAST_MESSAGE: Rep = Rep::new(-30, "Polkadot: Past message");
	/// A peer sent us a malformed message.
	pub const MALFORMED_MESSAGE: Rep = Rep::new(-500, "Polkadot: Malformed message");
	/// A peer sent us a wrongly signed message.
	pub const BAD_SIGNATURE: Rep = Rep::new(-500, "Polkadot: Bad signature");
	/// A peer sent us a bad neighbor packet.
	pub const BAD_NEIGHBOR_PACKET: Rep = Rep::new(-300, "Polkadot: Bad neighbor");
	/// A peer sent us an erasure chunk referring to a candidate that we are not aware of.
	pub const ORPHANED_ERASURE_CHUNK: Rep = Rep::new(-10, "An erasure chunk from unknown candidate");
	/// A peer sent us an erasure chunk that does not match candidate's erasure root.
	pub const ERASURE_CHUNK_WRONG_ROOT: Rep = Rep::new(-100, "Chunk doesn't match encoding root");
}

/// A gossip message.
#[derive(Encode, Decode, Clone, PartialEq)]
pub enum GossipMessage {
	/// A packet sent to a neighbor but not relayed.
	#[codec(index = "1")]
	Neighbor(VersionedNeighborPacket),
	/// An attestation-statement about the candidate.
	/// Non-candidate statements should only be sent to peers who are aware of the candidate.
	#[codec(index = "2")]
	Statement(GossipStatement),
	// TODO: https://github.com/paritytech/polkadot/issues/253
	/// A packet containing one of the erasure-coding chunks of one candidate.
	#[codec(index = "3")]
	ErasureChunk(ErasureChunkMessage),
	/// A PoV-block.
	#[codec(index = "255")]
	PoVBlock(GossipPoVBlock),
}

impl From<NeighborPacket> for GossipMessage {
	fn from(packet: NeighborPacket) -> Self {
		GossipMessage::Neighbor(VersionedNeighborPacket::V1(packet))
	}
}

impl From<GossipStatement> for GossipMessage {
	fn from(stmt: GossipStatement) -> Self {
		GossipMessage::Statement(stmt)
	}
}

impl From<GossipPoVBlock> for GossipMessage {
	fn from(pov: GossipPoVBlock) -> Self {
		GossipMessage::PoVBlock(pov)
	}
}

/// A gossip message containing a statement.
#[derive(Encode, Decode, Clone, PartialEq)]
pub struct GossipStatement {
	/// The block hash of the relay chain being referred to. In context, this should
	/// be a leaf.
	pub relay_chain_leaf: Hash,
	/// The signed statement being gossipped.
	pub signed_statement: SignedStatement,
}

impl GossipStatement {
	/// Create a new instance.
	pub fn new(relay_chain_leaf: Hash, signed_statement: SignedStatement) -> Self {
		Self {
			relay_chain_leaf,
			signed_statement,
		}
	}
}

/// A gossip message containing one erasure chunk of a candidate block.
/// For each chunk of block erasure encoding one of this messages is constructed.
#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub struct ErasureChunkMessage {
	/// The chunk itself.
	pub chunk: PrimitiveChunk,
	/// The hash of the candidate receipt of the block this chunk belongs to.
	pub candidate_hash: Hash,
}

impl From<ErasureChunkMessage> for GossipMessage {
	fn from(chk: ErasureChunkMessage) -> Self {
		GossipMessage::ErasureChunk(chk)
	}
}

/// A pov-block being gossipped. Should only be sent to peers aware of the candidate
/// referenced.
#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub struct GossipPoVBlock {
	/// The block hash of the relay chain being referred to. In context, this should
	/// be a leaf.
	pub relay_chain_leaf: Hash,
	/// The hash of some candidate localized to the same relay-chain leaf, whose
	/// pov-block is this block.
	pub candidate_hash: Hash,
	/// The pov-block itself.
	pub pov_block: PoVBlock,
}

/// A versioned neighbor message.
#[derive(Encode, Decode, Clone, PartialEq)]
pub enum VersionedNeighborPacket {
	#[codec(index = "1")]
	V1(NeighborPacket),
}

/// Contains information on which chain heads the peer is
/// accepting messages for.
#[derive(Encode, Decode, Clone, PartialEq)]
pub struct NeighborPacket {
	chain_heads: Vec<Hash>,
}

/// whether a block is known.
#[derive(Clone, Copy, PartialEq)]
pub enum Known {
	/// The block is a known leaf.
	Leaf,
	/// The block is known to be old.
	Old,
	/// The block is known to be bad.
	Bad,
}

/// Context to the underlying polkadot chain.
pub trait ChainContext: Send + Sync {
	/// Provide a closure which is invoked for every unrouted queue hash at a given leaf.
	fn leaf_unrouted_roots(
		&self,
		leaf: &Hash,
		with_queue_root: &mut dyn FnMut(&Hash),
	) -> Result<(), ClientError>;

	/// whether a block is known. If it's not, returns `None`.
	fn is_known(&self, block_hash: &Hash) -> Option<Known>;
}

impl<F, P> ChainContext for (F, P) where
	F: Fn(&Hash) -> Option<Known> + Send + Sync,
	P: Send + Sync + std::ops::Deref,
	P::Target: ProvideRuntimeApi<Block>,
	<P::Target as ProvideRuntimeApi<Block>>::Api: ParachainHost<Block, Error = ClientError>,
{
	fn is_known(&self, block_hash: &Hash) -> Option<Known> {
		(self.0)(block_hash)
	}

	fn leaf_unrouted_roots(
		&self,
		_leaf: &Hash,
		_with_queue_root: &mut dyn FnMut(&Hash),
	) -> Result<(), ClientError> {
		Ok(())
	}
}


/// Compute the gossip topic for attestations on the given parent hash.
pub(crate) fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

/// Compute the gossip topic for PoV blocks based on the given parent hash.
pub(crate) fn pov_block_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"pov-blocks");

	BlakeTwo256::hash(&v[..])
}

/// Register a gossip validator on the network service.
// NOTE: since RegisteredMessageValidator is meant to be a type-safe proof
// that we've actually done the registration, this should be the only way
// to construct it outside of tests.
pub fn register_validator<C: ChainContext + 'static>(
	service: Arc<NetworkService<Block, Hash>>,
	chain: C,
	executor: &impl sp_core::traits::SpawnNamed,
) -> RegisteredMessageValidator
{
	let s = service.clone();
	let report_handle = Box::new(move |peer: &PeerId, cost_benefit: ReputationChange| {
		if cost_benefit.value != 0 {
			s.report_peer(peer.clone(), cost_benefit);
		}
	});
	let validator = Arc::new(MessageValidator {
		report_handle,
		inner: RwLock::new(Inner {
			peers: HashMap::new(),
			attestation_view: Default::default(),
			availability_store: None,
			chain,
		})
	});

	let gossip_side = validator.clone();
	let gossip_engine = Arc::new(Mutex::new(sc_network_gossip::GossipEngine::new(
		service.clone(),
		POLKADOT_ENGINE_ID,
		POLKADOT_PROTOCOL_NAME,
		gossip_side,
	)));

	// Spawn gossip engine.
	//
	// Ideally this would not be spawned as an orphaned task, but polled by
	// `RegisteredMessageValidator` which in turn would be polled by a `ValidationNetwork`.
	{
		let gossip_engine = gossip_engine.clone();
		let fut = futures::future::poll_fn(move |cx| {
			gossip_engine.lock().poll_unpin(cx)
		});
		executor.spawn("polkadot-legacy-gossip-engine", fut.boxed());
	}

	RegisteredMessageValidator {
		inner: validator as _,
		service: Some(service),
		gossip_engine: Some(gossip_engine),
	}
}

#[derive(PartialEq)]
enum NewLeafAction {
	// (who, message)
	TargetedMessage(PeerId, GossipMessage),
}

/// Actions to take after noting a new block-DAG leaf.
///
/// This should be consumed by passing a consensus-gossip handle to `perform`.
#[must_use = "New chain-head gossip actions must be performed"]
pub struct NewLeafActions {
	actions: Vec<NewLeafAction>,
}

impl NewLeafActions {
	#[cfg(test)]
	pub fn new() -> Self {
		NewLeafActions { actions: Vec::new() }
	}

	/// Perform the queued actions, feeding into gossip.
	pub fn perform(
		self,
		gossip: &dyn crate::legacy::GossipService,
	) {
		for action in self.actions {
			match action {
				NewLeafAction::TargetedMessage(who, message)
					=> gossip.send_message(who, message),
			}
		}
	}
}

/// A registered message validator.
///
/// Create this using `register_validator`.
#[derive(Clone)]
pub struct RegisteredMessageValidator {
	inner: Arc<MessageValidator<dyn ChainContext>>,
	// Note: this is always `Some` in real code and `None` in tests.
	service: Option<Arc<NetworkService<Block, Hash>>>,
	// Note: this is always `Some` in real code and `None` in tests.
	gossip_engine: Option<Arc<Mutex<sc_network_gossip::GossipEngine<Block>>>>,
}

impl RegisteredMessageValidator {
	/// Register an availabilty store the gossip service can query.
	pub(crate) fn register_availability_store(&self, availability_store: av_store::Store) {
		self.inner.inner.write().availability_store = Some(availability_store);
	}

	/// Note that we perceive a new leaf of the block-DAG. We will notify our neighbors that
	/// we now accept parachain candidate attestations and incoming message queues
	/// relevant to this leaf.
	pub(crate) fn new_local_leaf(
		&self,
		validation: MessageValidationData,
	) -> NewLeafActions {
		// add an entry in attestation_view
		// prune any entries from attestation_view which are no longer leaves
		let mut inner = self.inner.inner.write();
		inner.attestation_view.new_local_leaf(validation);

		let mut actions = Vec::new();

		{
			let &mut Inner {
				ref chain,
				ref mut attestation_view,
				..
			} = &mut *inner;

			attestation_view.prune_old_leaves(|hash| match chain.is_known(hash) {
				Some(Known::Leaf) => true,
				_ => false,
			});
		}


		// send neighbor packets to peers
		inner.multicast_neighbor_packet(
			|who, message| actions.push(NewLeafAction::TargetedMessage(who.clone(), message))
		);

		NewLeafActions { actions }
	}

	pub(crate) fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		let topic_stream = if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().messages_for(topic)
		} else {
			log::error!("Called gossip_messages_for on a test engine");
			futures::channel::mpsc::channel(0).1
		};

		GossipMessageStream::new(topic_stream.boxed())
	}

	pub(crate) fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().gossip_message(
				topic,
				message.encode(),
				false,
			);
		} else {
			log::error!("Called gossip_message on a test engine");
		}
	}

	pub(crate) fn send_message(&self, who: PeerId, message: GossipMessage) {
		if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().send_message(vec![who], message.encode());
		} else {
			log::error!("Called send_message on a test engine");
		}
	}
}

impl GossipService for RegisteredMessageValidator {
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		RegisteredMessageValidator::gossip_messages_for(self, topic)
	}

	fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		RegisteredMessageValidator::gossip_message(self, topic, message)
	}

	fn send_message(&self, who: PeerId, message: GossipMessage) {
		RegisteredMessageValidator::send_message(self, who, message)
	}
}

/// The data needed for validating gossip messages.
#[derive(Default)]
pub(crate) struct MessageValidationData {
	/// The authorities' parachain validation keys at a block.
	pub(crate) authorities: Vec<ValidatorId>,
	/// The signing context.
	pub(crate) signing_context: SigningContext,
}

impl MessageValidationData {
	// check a statement's signature.
	fn check_statement(&self, statement: &SignedStatement) -> Result<(), ()> {
		let sender = match self.authorities.get(statement.sender as usize) {
			Some(val) => val,
			None => return Err(()),
		};

		let good = self.authorities.contains(&sender) &&
			::polkadot_validation::check_statement(
				&statement.statement,
				&statement.signature,
				sender.clone(),
				&self.signing_context,
			);

		if good {
			Ok(())
		} else {
			Err(())
		}
	}
}

#[derive(Default)]
struct PeerData {
	attestation: AttestationPeerData,
}

struct Inner<C: ?Sized> {
	peers: HashMap<PeerId, PeerData>,
	attestation_view: AttestationView,
	availability_store: Option<av_store::Store>,
	chain: C,
}

impl<C: ?Sized + ChainContext> Inner<C> {
	fn validate_neighbor_packet(&mut self, sender: &PeerId, packet: NeighborPacket)
		-> (GossipValidationResult<Hash>, ReputationChange, Vec<Hash>)
	{
		let chain_heads = packet.chain_heads;
		if chain_heads.len() > MAX_CHAIN_HEADS {
			(GossipValidationResult::Discard, cost::BAD_NEIGHBOR_PACKET, Vec::new())
		} else {
			let chain_heads: LeavesVec = chain_heads.into_iter().collect();
			let new_topics = if let Some(ref mut peer) = self.peers.get_mut(sender) {
				let new_leaves = peer.attestation.update_leaves(&chain_heads);
				let new_attestation_topics = new_leaves.iter().cloned().map(attestation_topic);
				let new_pov_block_topics = new_leaves.iter().cloned().map(pov_block_topic);

				new_attestation_topics.chain(new_pov_block_topics).collect()
			} else {
				Vec::new()
			};

			(GossipValidationResult::Discard, cost::NONE, new_topics)
		}
	}

	fn validate_erasure_chunk_packet(&mut self, msg: ErasureChunkMessage)
		-> (GossipValidationResult<Hash>, ReputationChange)
	{
		if let Some(store) = &self.availability_store {
			if let Some(receipt) = store.get_candidate(&msg.candidate_hash) {
				let chunk_hash = erasure::branch_hash(
					&receipt.commitments.erasure_root,
					&msg.chunk.proof,
					msg.chunk.index as usize
				);

				if chunk_hash != Ok(BlakeTwo256::hash(&msg.chunk.chunk)) {
					(
						GossipValidationResult::Discard,
						cost::ERASURE_CHUNK_WRONG_ROOT
					)
				} else {
					if let Some(awaited_chunks) = store.awaited_chunks() {
						let frontier_entry = av_store::AwaitedFrontierEntry {
							candidate_hash: msg.candidate_hash,
							relay_parent: receipt.relay_parent,
							validator_index: msg.chunk.index,
						};
						if awaited_chunks.contains(&frontier_entry) {
							let topic = crate::erasure_coding_topic(
								&msg.candidate_hash
							);

							return (
								GossipValidationResult::ProcessAndKeep(topic),
								benefit::NEW_ERASURE_CHUNK,
							);
						}
					}
					(GossipValidationResult::Discard, cost::NONE)
				}
			} else {
				(GossipValidationResult::Discard, cost::ORPHANED_ERASURE_CHUNK)
			}
		} else {
			(GossipValidationResult::Discard, cost::NONE)
		}
	}

	fn multicast_neighbor_packet<F: FnMut(&PeerId, GossipMessage)>(
		&self,
		mut send_neighbor_packet: F,
	) {
		let neighbor_packet = GossipMessage::from(NeighborPacket {
			chain_heads: self.attestation_view.neighbor_info().collect(),
		});

		for peer in self.peers.keys() {
			send_neighbor_packet(peer, neighbor_packet.clone())
		}
	}
}

/// An unregistered message validator. Register this with `register_validator`.
pub struct MessageValidator<C: ?Sized> {
	report_handle: Box<dyn Fn(&PeerId, ReputationChange) + Send + Sync>,
	inner: RwLock<Inner<C>>,
}

impl<C: ChainContext + ?Sized> MessageValidator<C> {
	#[cfg(test)]
	fn new_test(
		chain: C,
		report_handle: Box<dyn Fn(&PeerId, ReputationChange) + Send + Sync>,
	) -> Self where C: Sized {
		MessageValidator {
			report_handle,
			inner: RwLock::new(Inner {
				peers: HashMap::new(),
				attestation_view: Default::default(),
				availability_store: None,
				chain,
			}),
		}
	}

	fn report(&self, who: &PeerId, cost_benefit: ReputationChange) {
		(self.report_handle)(who, cost_benefit)
	}
}

impl<C: ChainContext + ?Sized> sc_network_gossip::Validator<Block> for MessageValidator<C> {
	fn new_peer(&self, _context: &mut dyn ValidatorContext<Block>, who: &PeerId, _role: ObservedRole) {
		let mut inner = self.inner.write();
		inner.peers.insert(who.clone(), PeerData::default());
	}

	fn peer_disconnected(&self, _context: &mut dyn ValidatorContext<Block>, who: &PeerId) {
		let mut inner = self.inner.write();
		inner.peers.remove(who);
	}

	fn validate(&self, context: &mut dyn ValidatorContext<Block>, sender: &PeerId, data: &[u8])
		-> GossipValidationResult<Hash>
	{
		let mut decode_data = data;
		let (res, cost_benefit) = match GossipMessage::decode(&mut decode_data) {
			Err(_) => (GossipValidationResult::Discard, cost::MALFORMED_MESSAGE),
			Ok(GossipMessage::Neighbor(VersionedNeighborPacket::V1(packet))) => {
				let (res, cb, topics) = self.inner.write().validate_neighbor_packet(sender, packet);
				for new_topic in topics {
					context.send_topic(sender, new_topic, false);
				}
				(res, cb)
			}
			Ok(GossipMessage::Statement(statement)) => {
				let (res, cb) = {
					let mut inner = self.inner.write();
					let inner = &mut *inner;
					inner.attestation_view.validate_statement_signature(statement, &inner.chain)
				};

				if let GossipValidationResult::ProcessAndKeep(ref topic) = res {
					context.broadcast_message(topic.clone(), data.to_vec(), false);
				}
				(res, cb)
			}
			Ok(GossipMessage::PoVBlock(pov_block)) => {
				let (res, cb) = {
					let mut inner = self.inner.write();
					let inner = &mut *inner;
					inner.attestation_view.validate_pov_block_message(&pov_block, &inner.chain)
				};

				if let GossipValidationResult::ProcessAndKeep(ref topic) = res {
					context.broadcast_message(topic.clone(), data.to_vec(), false);
				}

				(res, cb)
			}
			Ok(GossipMessage::ErasureChunk(chunk)) => {
				self.inner.write().validate_erasure_chunk_packet(chunk)
			}
		};

		self.report(sender, cost_benefit);
		res
	}

	fn message_expired<'a>(&'a self) -> Box<dyn FnMut(Hash, &[u8]) -> bool + 'a> {
		let inner = self.inner.read();

		Box::new(move |topic, _data| {
			// check that messages from this topic are considered live by one of our protocols.
			// everything else is expired
			let live = inner.attestation_view.is_topic_live(&topic);

			!live // = expired
		})
	}

	fn message_allowed<'a>(&'a self) -> Box<dyn FnMut(&PeerId, MessageIntent, &Hash, &[u8]) -> bool + 'a> {
		let mut inner = self.inner.write();
		Box::new(move |who, intent, topic, data| {
			let &mut Inner {
				ref mut peers,
				ref mut attestation_view,
				..
			} = &mut *inner;

			match intent {
				MessageIntent::PeriodicRebroadcast => return false,
				_ => {},
			}

			let attestation_head = attestation_view.topic_block(topic).map(|x| x.clone());
			let peer = peers.get_mut(who);

			match GossipMessage::decode(&mut &data[..]) {
				Ok(GossipMessage::Statement(ref statement)) => {
					// to allow statements, we need peer knowledge.
					let peer_knowledge = peer.and_then(move |p| attestation_head.map(|r| (p, r)))
						.and_then(|(p, r)| p.attestation.knowledge_at_mut(&r).map(|k| (k, r)));

					peer_knowledge.map_or(false, |(knowledge, attestation_head)| {
						statement.relay_chain_leaf == attestation_head
							&& attestation_view.statement_allowed(
								statement,
								knowledge,
							)
					})
				}
				Ok(GossipMessage::PoVBlock(ref pov_block)) => {
					// to allow pov-blocks, we need peer knowledge.
					let peer_knowledge = peer.and_then(move |p| attestation_head.map(|r| (p, r)))
						.and_then(|(p, r)| p.attestation.knowledge_at_mut(&r).map(|k| (k, r)));

					peer_knowledge.map_or(false, |(knowledge, attestation_head)| {
						pov_block.relay_chain_leaf == attestation_head
							&& attestation_view.pov_block_allowed(
								pov_block,
								knowledge,
							)
					})
				}
				_ => false,
			}
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sc_network_gossip::Validator as ValidatorT;
	use std::sync::mpsc;
	use parking_lot::Mutex;
	use polkadot_primitives::v0::{AbridgedCandidateReceipt, BlockData};
	use sp_core::sr25519::Signature as Sr25519Signature;
	use polkadot_validation::GenericStatement;

	#[derive(PartialEq, Clone, Debug)]
	enum ContextEvent {
		BroadcastTopic(Hash, bool),
		BroadcastMessage(Hash, Vec<u8>, bool),
		SendMessage(PeerId, Vec<u8>),
		SendTopic(PeerId, Hash, bool),
	}

	#[derive(Default)]
	struct MockValidatorContext {
		events: Vec<ContextEvent>,
	}

	impl MockValidatorContext {
		fn clear(&mut self) {
			self.events.clear()
		}
	}

	impl sc_network_gossip::ValidatorContext<Block> for MockValidatorContext {
		fn broadcast_topic(&mut self, topic: Hash, force: bool) {
			self.events.push(ContextEvent::BroadcastTopic(topic, force));
		}
		fn broadcast_message(&mut self, topic: Hash, message: Vec<u8>, force: bool) {
			self.events.push(ContextEvent::BroadcastMessage(topic, message, force));
		}
		fn send_message(&mut self, who: &PeerId, message: Vec<u8>) {
			self.events.push(ContextEvent::SendMessage(who.clone(), message));
		}
		fn send_topic(&mut self, who: &PeerId, topic: Hash, force: bool) {
			self.events.push(ContextEvent::SendTopic(who.clone(), topic, force));
		}
	}

	#[derive(Default)]
	struct TestChainContext {
		known_map: HashMap<Hash, Known>,
		ingress_roots: HashMap<Hash, Vec<Hash>>,
	}

	impl ChainContext for TestChainContext {
		fn is_known(&self, block_hash: &Hash) -> Option<Known> {
			self.known_map.get(block_hash).map(|x| x.clone())
		}

		fn leaf_unrouted_roots(&self, leaf: &Hash, with_queue_root: &mut dyn FnMut(&Hash))
			-> Result<(), sp_blockchain::Error>
		{
			for root in self.ingress_roots.get(leaf).into_iter().flat_map(|roots| roots) {
				with_queue_root(root)
			}

			Ok(())
		}
	}

	#[test]
	fn attestation_message_allowed() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, ObservedRole::Full);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let hash_a = [1u8; 32].into();
		let hash_b = [2u8; 32].into();
		let hash_c = [3u8; 32].into();

		let message = GossipMessage::from(NeighborPacket {
			chain_heads: vec![hash_a, hash_b],
		}).encode();
		let res = validator.validate(
			&mut validator_context,
			&peer_a,
			&message[..],
		);

		match res {
			GossipValidationResult::Discard => {},
			_ => panic!("wrong result"),
		}
		assert_eq!(
			validator_context.events,
			vec![
				ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_a), false),
				ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_b), false),

				ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_a), false),
				ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_b), false),
			],
		);

		validator_context.clear();

		let candidate_receipt = AbridgedCandidateReceipt::default();
		let statement = GossipMessage::Statement(GossipStatement {
			relay_chain_leaf: hash_a,
			signed_statement: SignedStatement {
				statement: GenericStatement::Candidate(candidate_receipt),
				signature: Sr25519Signature([255u8; 64]).into(),
				sender: 1,
			}
		});
		let encoded = statement.encode();

		let topic_a = attestation_topic(hash_a);
		let topic_b = attestation_topic(hash_b);
		let topic_c = attestation_topic(hash_c);

		// topic_a is in all 3 views -> succeed
		let mut validation_data = MessageValidationData::default();
		validation_data.signing_context.parent_hash = hash_a;
		validator.inner.write().attestation_view.new_local_leaf(validation_data);
		// topic_b is in the neighbor's view but not ours -> fail
		// topic_c is not in either -> fail

		{
			let mut message_allowed = validator.message_allowed();
			let intent = MessageIntent::Broadcast;
			assert!(message_allowed(&peer_a, intent, &topic_a, &encoded));
			assert!(!message_allowed(&peer_a, intent, &topic_b, &encoded));
			assert!(!message_allowed(&peer_a, intent, &topic_c, &encoded));
		}
	}

	#[test]
	fn too_many_chain_heads_is_report() {
		let (tx, rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, ObservedRole::Full);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let chain_heads = (0..MAX_CHAIN_HEADS+1).map(|i| [i as u8; 32].into()).collect();

		let message = GossipMessage::from(NeighborPacket {
			chain_heads,
		}).encode();
		let res = validator.validate(
			&mut validator_context,
			&peer_a,
			&message[..],
		);

		match res {
			GossipValidationResult::Discard => {},
			_ => panic!("wrong result"),
		}
		assert_eq!(
			validator_context.events,
			Vec::new(),
		);

		drop(validator);

		assert_eq!(rx.iter().collect::<Vec<_>>(), vec![(peer_a, cost::BAD_NEIGHBOR_PACKET)]);
	}

	#[test]
	fn statement_only_sent_when_candidate_known() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, ObservedRole::Full);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let hash_a = [1u8; 32].into();
		let hash_b = [2u8; 32].into();

		let message = GossipMessage::from(NeighborPacket {
			chain_heads: vec![hash_a, hash_b],
		}).encode();

		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&message[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				vec![
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_b), false),

					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_b), false),
				],
			);

				validator_context.clear();
		}

		let mut validation_data = MessageValidationData::default();
		validation_data.signing_context.parent_hash = hash_a;
		validator.inner.write().attestation_view.new_local_leaf(validation_data);
	}

	#[test]
	fn pov_block_message_allowed() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, ObservedRole::Full);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let hash_a = [1u8; 32].into();
		let hash_b = [2u8; 32].into();

		let message = GossipMessage::from(NeighborPacket {
			chain_heads: vec![hash_a, hash_b],
		}).encode();

		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&message[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				vec![
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_b), false),

					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_b), false),
				],
			);

			validator_context.clear();
		}

		let topic_a = pov_block_topic(hash_a);
		let c_hash = [99u8; 32].into();

		let pov_block = PoVBlock {
			block_data: BlockData(vec![1, 2, 3]),
		};

		let pov_block_hash = pov_block.hash();

		let message = GossipMessage::PoVBlock(GossipPoVBlock {
			relay_chain_leaf: hash_a,
			candidate_hash: c_hash,
			pov_block,
		});
		let encoded = message.encode();
		let mut validation_data = MessageValidationData::default();
		validation_data.signing_context.parent_hash = hash_a;
		validator.inner.write().attestation_view.new_local_leaf(validation_data);

		{
			let mut message_allowed = validator.message_allowed();
			assert!(!message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded[..]));
		}

		validator
			.inner
			.write()
			.peers
			.get_mut(&peer_a)
			.unwrap()
			.attestation
			.note_aware_under_leaf(
				&hash_a,
				c_hash,
				attestation::CandidateMeta { pov_block_hash },
			);

		{
			let mut message_allowed = validator.message_allowed();
			assert!(message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded[..]));
		}
	}

	#[test]
	fn validate_pov_block_message() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, ObservedRole::Full);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let hash_a = [1u8; 32].into();
		let hash_b = [2u8; 32].into();

		let message = GossipMessage::from(NeighborPacket {
			chain_heads: vec![hash_a, hash_b],
		}).encode();

		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&message[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				vec![
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), attestation_topic(hash_b), false),

					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_a), false),
					ContextEvent::SendTopic(peer_a.clone(), pov_block_topic(hash_b), false),
				],
			);

			validator_context.clear();
		}

		let pov_topic = pov_block_topic(hash_a);

		let pov_block = PoVBlock {
			block_data: BlockData(vec![1, 2, 3]),
		};

		let pov_block_hash = pov_block.hash();
		let c_hash = [99u8; 32].into();

		let message = GossipMessage::PoVBlock(GossipPoVBlock {
			relay_chain_leaf: hash_a,
			candidate_hash: c_hash,
			pov_block,
		});

		let bad_message = GossipMessage::PoVBlock(GossipPoVBlock {
			relay_chain_leaf: hash_a,
			candidate_hash: c_hash,
			pov_block: PoVBlock {
				block_data: BlockData(vec![4, 5, 6]),
			},
		});

		let encoded = message.encode();
		let bad_encoded = bad_message.encode();

		let mut validation_data = MessageValidationData::default();
		validation_data.signing_context.parent_hash = hash_a;
		validator.inner.write().attestation_view.new_local_leaf(validation_data);

		// before sending `Candidate` message, neither are allowed.
		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&encoded[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				Vec::new(),
			);

			validator_context.clear();
		}

		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&bad_encoded[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				Vec::new(),
			);

			validator_context.clear();
		}

		validator.inner.write().attestation_view.note_aware_under_leaf(
			&hash_a,
			c_hash,
			attestation::CandidateMeta { pov_block_hash },
		);

		// now the good message passes and the others not.
		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&encoded[..],
			);

			match res {
				GossipValidationResult::ProcessAndKeep(topic) => assert_eq!(topic,pov_topic),
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				vec![
					ContextEvent::BroadcastMessage(pov_topic, encoded.clone(), false),
				],
			);

			validator_context.clear();
		}

		{
			let res = validator.validate(
				&mut validator_context,
				&peer_a,
				&bad_encoded[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}
			assert_eq!(
				validator_context.events,
				Vec::new(),
			);

			validator_context.clear();
		}
	}
}
