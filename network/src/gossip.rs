// Copyright 2019 Parity Technologies (UK) Ltd.
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

use sr_primitives::{generic::BlockId, traits::ProvideRuntimeApi};
use substrate_client::error::Error as ClientError;
use substrate_network::{config::Roles, PeerId};
use substrate_network::consensus_gossip::{
	self as network_gossip, ValidationResult as GossipValidationResult,
	ValidatorContext, MessageIntent, ConsensusMessage,
};
use polkadot_validation::SignedStatement;
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::{ParachainHost, ValidatorId, Message as ParachainMessage};
use codec::{Decode, Encode};

use std::collections::HashMap;
use std::sync::Arc;

use arrayvec::ArrayVec;
use parking_lot::RwLock;
use log::warn;

use super::PolkadotNetworkService;
use crate::router::attestation_topic;

use attestation::{View as AttestationView, PeerData as AttestationPeerData};
use message_routing::{View as MessageRoutingView};

mod attestation;
mod message_routing;

/// The engine ID of the polkadot attestation system.
pub const POLKADOT_ENGINE_ID: sr_primitives::ConsensusEngineId = *b"dot1";

// arbitrary; in practice this should not be more than 2.
pub(crate) const MAX_CHAIN_HEADS: usize = 5;

/// Type alias for a bounded vector of leaves.
pub type LeavesVec = ArrayVec<[Hash; MAX_CHAIN_HEADS]>;

mod benefit {
	/// When a peer sends us a previously-unknown candidate statement.
	pub const NEW_CANDIDATE: i32 = 100;
	/// When a peer sends us a previously-unknown attestation.
	pub const NEW_ATTESTATION: i32 = 50;
	/// When a peer sends us a previously-unknown message packet.
	pub const NEW_ICMP_MESSAGES: i32 = 50;
}

mod cost {
	/// A peer sent us an attestation and we don't know the candidate.
	pub const ATTESTATION_NO_CANDIDATE: i32 = -100;
	/// A peer sent us a statement we consider in the future.
	pub const FUTURE_MESSAGE: i32 = -100;
	/// A peer sent us a statement from the past.
	pub const PAST_MESSAGE: i32 = -30;
	/// A peer sent us a malformed message.
	pub const MALFORMED_MESSAGE: i32 = -500;
	/// A peer sent us a wrongly signed message.
	pub const BAD_SIGNATURE: i32 = -500;
	/// A peer sent us a bad neighbor packet.
	pub const BAD_NEIGHBOR_PACKET: i32 = -300;
	/// A peer sent us an ICMP queue we haven't advertised a need for.
	pub const UNNEEDED_ICMP_MESSAGES: i32 = -100;

	/// A peer sent us an ICMP queue with a bad root.
	pub fn icmp_messages_root_mismatch(n_messages: usize) -> i32 {
		const PER_MESSAGE: i32 = -150;

		(0..n_messages).map(|_| PER_MESSAGE).sum()
	}
}

/// A gossip message.
#[derive(Encode, Decode, Clone)]
pub enum GossipMessage {
	/// A packet sent to a neighbor but not relayed.
	#[codec(index = "1")]
	Neighbor(VersionedNeighborPacket),
	/// An attestation-statement about the candidate.
	/// Non-candidate statements should only be sent to peers who are aware of the candidate.
	#[codec(index = "2")]
	Statement(GossipStatement),
	/// A packet of messages from one parachain to another.
	#[codec(index = "3")]
	ParachainMessages(GossipParachainMessages),
	// TODO: https://github.com/paritytech/polkadot/issues/253
	// erasure-coded chunks.
}

impl GossipMessage {
	fn to_consensus_message(&self) -> ConsensusMessage {
		ConsensusMessage {
			data: self.encode(),
			engine_id: POLKADOT_ENGINE_ID,
		}
	}
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

impl From<GossipParachainMessages> for GossipMessage {
	fn from(messages: GossipParachainMessages) -> Self {
		GossipMessage::ParachainMessages(messages)
	}
}

/// A gossip message containing a statement.
#[derive(Encode, Decode, Clone)]
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

/// A packet of messages from one parachain to another.
///
/// These are all the messages posted from one parachain to another during the
/// execution of a single parachain block. Since this parachain block may have been
/// included in many forks of the relay chain, there is no relay-chain leaf parameter.
#[derive(Encode, Decode, Clone)]
pub struct GossipParachainMessages {
	/// The root of the message queue.
	pub queue_root: Hash,
	/// The messages themselves.
	pub messages: Vec<ParachainMessage>,
}

impl GossipParachainMessages {
	// confirms that the queue-root in the struct correctly matches
	// the messages.
	fn queue_root_is_correct(&self) -> bool {
		let root = polkadot_validation::message_queue_root(
			self.messages.iter().map(|m| &m.0)
		);
		root == self.queue_root
	}
}

/// A versioned neighbor message.
#[derive(Encode, Decode, Clone)]
pub enum VersionedNeighborPacket {
	#[codec(index = "1")]
	V1(NeighborPacket),
}

/// Contains information on which chain heads the peer is
/// accepting messages for.
#[derive(Encode, Decode, Clone)]
pub struct NeighborPacket {
	chain_heads: Vec<Hash>,
}

/// whether a block is known.
#[derive(Clone, Copy)]
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
	P::Target: ProvideRuntimeApi,
	<P::Target as ProvideRuntimeApi>::Api: ParachainHost<Block>,
{
	fn is_known(&self, block_hash: &Hash) -> Option<Known> {
		(self.0)(block_hash)
	}

	fn leaf_unrouted_roots(
		&self,
		&leaf: &Hash,
		with_queue_root: &mut dyn FnMut(&Hash),
	) -> Result<(), ClientError> {
		let api = self.1.runtime_api();

		let leaf_id = BlockId::Hash(leaf);
		let active_parachains = api.active_parachains(&leaf_id)?;

		// TODO: https://github.com/paritytech/polkadot/issues/467
		for (para_id, _) in active_parachains {
			if let Some(ingress) = api.ingress(&leaf_id, para_id, None)? {
				for (_height, _from, queue_root) in ingress.iter() {
					with_queue_root(queue_root);
				}
			}
		}

		Ok(())
	}
}

/// Register a gossip validator on the network service.
// NOTE: since RegisteredMessageValidator is meant to be a type-safe proof
// that we've actually done the registration, this should be the only way
// to construct it outside of tests.
pub fn register_validator<C: ChainContext + 'static>(
	service: Arc<PolkadotNetworkService>,
	chain: C,
) -> RegisteredMessageValidator
{
	let s = service.clone();
	let report_handle = Box::new(move |peer: &PeerId, cost_benefit| {
		s.report_peer(peer.clone(), cost_benefit);
	});
	let validator = Arc::new(MessageValidator {
		report_handle,
		inner: RwLock::new(Inner {
			peers: HashMap::new(),
			attestation_view: Default::default(),
			message_routing_view: Default::default(),
			chain,
		})
	});

	let gossip_side = validator.clone();
	service.with_gossip(|gossip, ctx|
		gossip.register_validator(ctx, POLKADOT_ENGINE_ID, gossip_side)
	);

	RegisteredMessageValidator { inner: validator as _ }
}

#[derive(PartialEq)]
enum NewLeafAction {
	// (who, message)
	TargetedMessage(PeerId, ConsensusMessage),
	// (topic, message)
	Multicast(Hash, ConsensusMessage),
}

/// Actions to take after noting a new block-DAG leaf.
///
/// This should be consumed by passing a consensus-gossip handle to `perform`.
#[must_use = "New chain-head gossip actions must be performed"]
pub struct NewLeafActions {
	actions: Vec<NewLeafAction>,
}

impl NewLeafActions {
	/// Perform the queued actions, feeding into gossip.
	pub fn perform(
		self,
		gossip: &mut dyn crate::GossipService,
		ctx: &mut dyn substrate_network::Context<Block>,
	) {
		for action in self.actions {
			match action {
				NewLeafAction::TargetedMessage(who, message)
					=> gossip.send_message(ctx, &who, message),
				NewLeafAction::Multicast(topic, message)
					=> gossip.multicast(ctx, &topic, message),
			}
		}
	}
}

/// Register a gossip validator for a non-authority node.
pub fn register_non_authority_validator(service: Arc<PolkadotNetworkService>) {
	service.with_gossip(|gossip, ctx|
		gossip.register_validator(
			ctx,
			POLKADOT_ENGINE_ID,
			Arc::new(substrate_network::consensus_gossip::DiscardAll)),
	);
}

/// A registered message validator.
///
/// Create this using `register_validator`.
#[derive(Clone)]
pub struct RegisteredMessageValidator {
	inner: Arc<MessageValidator<dyn ChainContext>>,
}

impl RegisteredMessageValidator {
	#[cfg(test)]
	pub(crate) fn new_test<C: ChainContext + 'static>(
		chain: C,
		report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	) -> Self {
		let validator = Arc::new(MessageValidator::new_test(chain, report_handle));

		RegisteredMessageValidator { inner: validator as _ }
	}

	/// Note that we perceive a new leaf of the block-DAG. We will notify our neighbors that
	/// we now accept parachain candidate attestations and incoming message queues
	/// relevant to this leaf.
	pub(crate) fn new_local_leaf(
		&self,
		relay_chain_leaf: Hash,
		validation: MessageValidationData,
		lookup_queue_by_root: impl Fn(&Hash) -> Option<Vec<ParachainMessage>>,
	) -> NewLeafActions {
		// add an entry in attestation_view
		// prune any entries from attestation_view which are no longer leaves
		let mut inner = self.inner.inner.write();
		inner.attestation_view.new_local_leaf(relay_chain_leaf, validation);

		let mut actions = Vec::new();

		{
			let &mut Inner {
				ref chain,
				ref mut attestation_view,
				ref mut message_routing_view,
				..
			} = &mut *inner;

			attestation_view.prune_old_leaves(|hash| match chain.is_known(hash) {
				Some(Known::Leaf) => true,
				_ => false,
			});

			if let Err(e) = message_routing_view.update_leaves(chain, attestation_view.neighbor_info()) {
				warn!("Unable to fully update leaf-state: {:?}", e);
			}
		}


		// send neighbor packets to peers
		inner.multicast_neighbor_packet(
			|who, message| actions.push(NewLeafAction::TargetedMessage(who.clone(), message))
		);

		// feed any new unrouted queues into the propagation pool.
		inner.message_routing_view.sweep_unknown_queues(|topic, queue_root|
			match lookup_queue_by_root(queue_root) {
				Some(messages) => {
					let message = GossipMessage::from(GossipParachainMessages {
						queue_root: *queue_root,
						messages,
					}).to_consensus_message();

					actions.push(NewLeafAction::Multicast(*topic, message));

					true
				}
				None => false,
			}
		);

		NewLeafActions { actions }
	}
}

/// The data needed for validating gossip messages.
#[derive(Default)]
pub(crate) struct MessageValidationData {
	/// The authorities' parachain validation keys at a block.
	pub(crate) authorities: Vec<ValidatorId>,
}

impl MessageValidationData {
	// check a statement's signature.
	fn check_statement(&self, relay_chain_leaf: &Hash, statement: &SignedStatement) -> Result<(), ()> {
		let sender = match self.authorities.get(statement.sender as usize) {
			Some(val) => val,
			None => return Err(()),
		};

		let good = self.authorities.contains(&sender) &&
			::polkadot_validation::check_statement(
				&statement.statement,
				&statement.signature,
				sender.clone(),
				relay_chain_leaf,
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

impl PeerData {
	fn leaves(&self) -> impl Iterator<Item = &Hash> {
		self.attestation.leaves()
	}
}

struct Inner<C: ?Sized> {
	peers: HashMap<PeerId, PeerData>,
	attestation_view: AttestationView,
	message_routing_view: MessageRoutingView,
	chain: C,
}

impl<C: ?Sized + ChainContext> Inner<C> {
	fn validate_neighbor_packet(&mut self, sender: &PeerId, packet: NeighborPacket)
		-> (GossipValidationResult<Hash>, i32, Vec<Hash>)
	{
		let chain_heads = packet.chain_heads;
		if chain_heads.len() > MAX_CHAIN_HEADS {
			(GossipValidationResult::Discard, cost::BAD_NEIGHBOR_PACKET, Vec::new())
		} else {
			let chain_heads: LeavesVec = chain_heads.into_iter().collect();
			let new_topics = if let Some(ref mut peer) = self.peers.get_mut(sender) {
				let new_leaves = peer.attestation.update_leaves(&chain_heads);
				let new_attestation_topics = new_leaves.iter().cloned().map(attestation_topic);

				// find all topics which are from the intersection of our leaves with the peer's
				// new leaves.
				let new_message_routing_topics = self.message_routing_view.intersection_topics(&new_leaves);

				new_attestation_topics.chain(new_message_routing_topics).collect()
			} else {
				Vec::new()
			};

			(GossipValidationResult::Discard, 0, new_topics)
		}
	}

	fn multicast_neighbor_packet<F: FnMut(&PeerId, ConsensusMessage)>(
		&self,
		mut send_neighbor_packet: F,
	) {
		let neighbor_packet = GossipMessage::from(NeighborPacket {
			chain_heads: self.attestation_view.neighbor_info().collect(),
		}).to_consensus_message();

		for peer in self.peers.keys() {
			send_neighbor_packet(peer, neighbor_packet.clone())
		}
	}
}

/// An unregistered message validator. Register this with `register_validator`.
pub struct MessageValidator<C: ?Sized> {
	report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	inner: RwLock<Inner<C>>,
}

impl<C: ChainContext + ?Sized> MessageValidator<C> {
	#[cfg(test)]
	fn new_test(
		chain: C,
		report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	) -> Self where C: Sized {
		MessageValidator {
			report_handle,
			inner: RwLock::new(Inner {
				peers: HashMap::new(),
				attestation_view: Default::default(),
				message_routing_view: Default::default(),
				chain,
			}),
		}
	}

	fn report(&self, who: &PeerId, cost_benefit: i32) {
		(self.report_handle)(who, cost_benefit)
	}
}

impl<C: ChainContext + ?Sized> network_gossip::Validator<Block> for MessageValidator<C> {
	fn new_peer(&self, _context: &mut dyn ValidatorContext<Block>, who: &PeerId, _roles: Roles) {
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
			Ok(GossipMessage::ParachainMessages(messages)) => {
				let (res, cb) = {
					let mut inner = self.inner.write();
					let inner = &mut *inner;
					inner.message_routing_view.validate_queue_and_note_known(&messages)
				};

				if let GossipValidationResult::ProcessAndKeep(ref topic) = res {
					context.broadcast_message(topic.clone(), data.to_vec(), false);
				}
				(res, cb)
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
			let live = inner.attestation_view.is_topic_live(&topic)
				|| !inner.message_routing_view.is_topic_live(&topic);

			!live // = expired
		})
	}

	fn message_allowed<'a>(&'a self) -> Box<dyn FnMut(&PeerId, MessageIntent, &Hash, &[u8]) -> bool + 'a> {
		let mut inner = self.inner.write();
		Box::new(move |who, intent, topic, data| {
			let &mut Inner {
				ref mut peers,
				ref mut attestation_view,
				ref mut message_routing_view,
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
						attestation_view.statement_allowed(
							statement,
							&attestation_head,
							knowledge,
						)
					})
				}
				Ok(GossipMessage::ParachainMessages(_)) => match peer {
					None => false,
					Some(peer) => {
						let their_leaves: LeavesVec = peer.leaves().cloned().collect();
						message_routing_view.allowed_intersecting(&their_leaves, topic)
					}
				}
				_ => false,
			}
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_network::consensus_gossip::Validator as ValidatorT;
	use std::sync::mpsc;
	use parking_lot::Mutex;
	use polkadot_primitives::parachain::{CandidateReceipt, HeadData};
	use substrate_primitives::crypto::UncheckedInto;
	use substrate_primitives::sr25519::{Public as Sr25519Public, Signature as Sr25519Signature};
	use polkadot_validation::GenericStatement;
	use super::message_routing::queue_topic;

	use crate::tests::TestChainContext;

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

	impl network_gossip::ValidatorContext<Block> for MockValidatorContext {
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

	impl NewLeafActions {
		fn has_message(&self, who: PeerId, message: ConsensusMessage) -> bool {
			let x = NewLeafAction::TargetedMessage(who, message);
			self.actions.iter().find(|&m| m == &x).is_some()
		}

		fn has_multicast(&self, topic: Hash, message: ConsensusMessage) -> bool {
			let x = NewLeafAction::Multicast(topic, message);
			self.actions.iter().find(|&m| m == &x).is_some()
		}
	}

	fn validator_id(raw: [u8; 32]) -> ValidatorId {
		Sr25519Public::from_raw(raw).into()
	}

	#[test]
	fn message_allowed() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, Roles::FULL);
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
			],
		);

		validator_context.clear();

		let candidate_receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: [255; 32].unchecked_into(),
			head_data: HeadData(vec![9, 9, 9]),
			signature: Default::default(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [20u8; 32].into(),
			upward_messages: Vec::new(),
		};

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
		validator.inner.write().attestation_view.new_local_leaf(hash_a, MessageValidationData::default());
		// topic_b is in the neighbor's view but not ours -> fail
		// topic_c is not in either -> fail

		{
			let mut message_allowed = validator.message_allowed();
			assert!(message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded));
			assert!(!message_allowed(&peer_a, MessageIntent::Broadcast, &topic_b, &encoded));
			assert!(!message_allowed(&peer_a, MessageIntent::Broadcast, &topic_c, &encoded));
		}
	}

	#[test]
	fn too_many_chain_heads_is_report() {
		let (tx, rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, Roles::FULL);
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
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			TestChainContext::default(),
			report_handle,
		);

		let peer_a = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.new_peer(&mut validator_context, &peer_a, Roles::FULL);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let hash_a = [1u8; 32].into();
		let hash_b = [2u8; 32].into();

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
			],
		);

		validator_context.clear();

		let topic_a = attestation_topic(hash_a);
		let c_hash = [99u8; 32].into();

		let statement = GossipMessage::Statement(GossipStatement {
			relay_chain_leaf: hash_a,
			signed_statement: SignedStatement {
				statement: GenericStatement::Valid(c_hash),
				signature: Sr25519Signature([255u8; 64]).into(),
				sender: 1,
			}
		});
		let encoded = statement.encode();
		validator.inner.write().attestation_view.new_local_leaf(hash_a, MessageValidationData::default());

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
			.note_aware_under_leaf(&hash_a, c_hash);
		{
			let mut message_allowed = validator.message_allowed();
			assert!(message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded[..]));
		}
	}

	#[test]
	fn multicasts_icmp_queues_when_building_on_new_leaf() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());

		let hash_a = [1u8; 32].into();
		let root_a = [11u8; 32].into();
		let root_a_topic = queue_topic(root_a);

		let root_a_messages = vec![
			ParachainMessage(vec![1, 2, 3]),
			ParachainMessage(vec![4, 5, 6]),
		];

		let chain = {
			let mut chain = TestChainContext::default();
			chain.known_map.insert(hash_a, Known::Leaf);
			chain.ingress_roots.insert(hash_a, vec![root_a]);
			chain
		};

		let validator = RegisteredMessageValidator::new_test(chain, report_handle);

		let authorities: Vec<ValidatorId> = vec![validator_id([0; 32]), validator_id([10; 32])];

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.inner.new_peer(&mut validator_context, &peer_a, Roles::FULL);
		validator.inner.new_peer(&mut validator_context, &peer_b, Roles::FULL);
		assert!(validator_context.events.is_empty());
		validator_context.clear();


		{
			let message = GossipMessage::from(NeighborPacket {
				chain_heads: vec![hash_a],
			}).encode();
			let res = validator.inner.validate(
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
				],
			);
		}

		// ensure that we attempt to multicast all relevant queues after noting a leaf.
		{
			let actions = validator.new_local_leaf(
				hash_a,
				MessageValidationData { authorities },
				|root| if root == &root_a {
					Some(root_a_messages.clone())
				} else {
					None
				},
			);

			assert!(actions.has_message(peer_a.clone(), GossipMessage::from(NeighborPacket {
				chain_heads: vec![hash_a],
			}).to_consensus_message()));

			assert!(actions.has_multicast(root_a_topic, GossipMessage::from(GossipParachainMessages {
				queue_root: root_a,
				messages: root_a_messages.clone(),
			}).to_consensus_message()));
		}

		// ensure that we are allowed to multicast to a peer with same chain head,
		// but not to one without.
		{
			let message = GossipMessage::from(GossipParachainMessages {
				queue_root: root_a,
				messages: root_a_messages.clone(),
			}).encode();

			let mut allowed = validator.inner.message_allowed();
			assert!(allowed(&peer_a, MessageIntent::Broadcast, &root_a_topic, &message[..]));
			assert!(!allowed(&peer_b, MessageIntent::Broadcast, &root_a_topic, &message[..]));
		}
	}

	#[test]
	fn multicasts_icmp_queues_on_neighbor_update() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());

		let hash_a = [1u8; 32].into();
		let root_a = [11u8; 32].into();
		let root_a_topic = queue_topic(root_a);

		let root_a_messages = vec![
			ParachainMessage(vec![1, 2, 3]),
			ParachainMessage(vec![4, 5, 6]),
		];

		let chain = {
			let mut chain = TestChainContext::default();
			chain.known_map.insert(hash_a, Known::Leaf);
			chain.ingress_roots.insert(hash_a, vec![root_a]);
			chain
		};

		let validator = RegisteredMessageValidator::new_test(chain, report_handle);

		let authorities: Vec<ValidatorId> = vec![validator_id([0; 32]), validator_id([10; 32])];

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let mut validator_context = MockValidatorContext::default();
		validator.inner.new_peer(&mut validator_context, &peer_a, Roles::FULL);
		validator.inner.new_peer(&mut validator_context, &peer_b, Roles::FULL);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		// ensure that we attempt to multicast all relevant queues after noting a leaf.
		{
			let actions = validator.new_local_leaf(
				hash_a,
				MessageValidationData { authorities },
				|root| if root == &root_a {
					Some(root_a_messages.clone())
				} else {
					None
				},
			);

			assert!(actions.has_message(peer_a.clone(), GossipMessage::from(NeighborPacket {
				chain_heads: vec![hash_a],
			}).to_consensus_message()));

			assert!(actions.has_multicast(root_a_topic, GossipMessage::from(GossipParachainMessages {
				queue_root: root_a,
				messages: root_a_messages.clone(),
			}).to_consensus_message()));
		}

		// ensure that we are not allowed to multicast to either peer, as they
		// don't have the chain head.
		{
			let message = GossipMessage::from(GossipParachainMessages {
				queue_root: root_a,
				messages: root_a_messages.clone(),
			}).encode();

			let mut allowed = validator.inner.message_allowed();
			assert!(!allowed(&peer_a, MessageIntent::Broadcast, &root_a_topic, &message[..]));
			assert!(!allowed(&peer_b, MessageIntent::Broadcast, &root_a_topic, &message[..]));
		}

		// peer A gets updated to the chain head. now we'll attempt to broadcast
		// all queues to it.
		{
			let message = GossipMessage::from(NeighborPacket {
				chain_heads: vec![hash_a],
			}).encode();
			let res = validator.inner.validate(
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
					ContextEvent::SendTopic(peer_a.clone(), root_a_topic, false),
				],
			);
		}

		// ensure that we are allowed to multicast to a peer with same chain head,
		// but not to one without.
		{
			let message = GossipMessage::from(GossipParachainMessages {
				queue_root: root_a,
				messages: root_a_messages.clone(),
			}).encode();

			let mut allowed = validator.inner.message_allowed();
			assert!(allowed(&peer_a, MessageIntent::Broadcast, &root_a_topic, &message[..]));
			assert!(!allowed(&peer_b, MessageIntent::Broadcast, &root_a_topic, &message[..]));
		}
	}

	#[test]
	fn accepts_needed_unknown_icmp_message_queue() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());

		let hash_a = [1u8; 32].into();
		let root_a_messages = vec![
			ParachainMessage(vec![1, 2, 3]),
			ParachainMessage(vec![4, 5, 6]),
		];
		let not_root_a_messages = vec![
			ParachainMessage(vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1]),
			ParachainMessage(vec![4, 5, 6]),
		];

		let root_a = polkadot_validation::message_queue_root(
			root_a_messages.iter().map(|m| &m.0)
		);
		let not_root_a = [69u8; 32].into();
		let root_a_topic = queue_topic(root_a);

		let chain = {
			let mut chain = TestChainContext::default();
			chain.known_map.insert(hash_a, Known::Leaf);
			chain.ingress_roots.insert(hash_a, vec![root_a]);
			chain
		};

		let validator = RegisteredMessageValidator::new_test(chain, report_handle);

		let authorities: Vec<ValidatorId> = vec![validator_id([0; 32]), validator_id([10; 32])];

		let peer_a = PeerId::random();
		let mut validator_context = MockValidatorContext::default();

		validator.inner.new_peer(&mut validator_context, &peer_a, Roles::FULL);
		assert!(validator_context.events.is_empty());
		validator_context.clear();

		let queue_messages = GossipMessage::from(GossipParachainMessages {
			queue_root: root_a,
			messages: root_a_messages.clone(),
		}).to_consensus_message();

		let not_queue_messages = GossipMessage::from(GossipParachainMessages {
			queue_root: root_a,
			messages: not_root_a_messages.clone(),
		}).encode();

		let queue_messages_wrong_root = GossipMessage::from(GossipParachainMessages {
			queue_root: not_root_a,
			messages: root_a_messages.clone(),
		}).encode();

		// ensure that we attempt to multicast all relevant queues after noting a leaf.
		{
			let actions = validator.new_local_leaf(
				hash_a,
				MessageValidationData { authorities },
				|_root| None,
			);

			assert!(actions.has_message(peer_a.clone(), GossipMessage::from(NeighborPacket {
				chain_heads: vec![hash_a],
			}).to_consensus_message()));

			// we don't know this queue! no broadcast :(
			assert!(!actions.has_multicast(root_a_topic, queue_messages.clone()));
		}

		// rejects right queue with unknown root.
		{
			let res = validator.inner.validate(
				&mut validator_context,
				&peer_a,
				&queue_messages_wrong_root[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}

			assert_eq!(validator_context.events, Vec::new());
		}

		// rejects bad queue.
		{
			let res = validator.inner.validate(
				&mut validator_context,
				&peer_a,
				&not_queue_messages[..],
			);

			match res {
				GossipValidationResult::Discard => {},
				_ => panic!("wrong result"),
			}

			assert_eq!(validator_context.events, Vec::new());
		}

		// accepts the right queue.
		{
			let res = validator.inner.validate(
				&mut validator_context,
				&peer_a,
				&queue_messages.data[..],
			);

			match res {
				GossipValidationResult::ProcessAndKeep(topic) if topic == root_a_topic => {},
				_ => panic!("wrong result"),
			}

			assert_eq!(validator_context.events, vec![
				ContextEvent::BroadcastMessage(root_a_topic, queue_messages.data.clone(), false),
			]);
		}
	}
}
