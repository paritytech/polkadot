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

//! Gossip messages and the message validator

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
pub const POLKADOT_ENGINE_ID: sr_primitives::ConsensusEngineId = [b'd', b'o', b't', b'1'];

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

impl From<GossipStatement> for GossipMessage {
	fn from(stmt: GossipStatement) -> Self {
		GossipMessage::Statement(stmt)
	}
}

/// A gossip message containing a statement.
#[derive(Encode, Decode, Clone)]
pub struct GossipStatement {
	/// The relay chain parent hash.
	pub relay_parent: Hash,
	/// The signed statement being gossipped.
	pub signed_statement: SignedStatement,
}

impl GossipStatement {
	/// Create a new instance.
	pub fn new(relay_parent: Hash, signed_statement: SignedStatement) -> Self {
		Self {
			relay_parent,
			signed_statement,
		}
	}
}

/// A packet of messages from one parachain to another.
///
/// These are all the messages posted from one parachain to another during the
/// execution of a single parachain block. Since this parachain block may have been
/// included in many forks of the relay chain, there is no relay-parent parameter.
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

		for para_id in active_parachains {
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
///
/// This returns a `RegisteredMessageValidator`
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

enum NewSessionAction {
	// (who, message)
	TargetedMessage(PeerId, ConsensusMessage),
	// (topic, message)
	Multicast(Hash, ConsensusMessage),
}

/// Actions to take after noting a new live attestation session.
///
/// This should be consumed by passing a consensus-gossip handle to `perform`.
#[must_use = "New session gossip actions must be performed"]
pub struct NewSessionActions {
	actions: Vec<NewSessionAction>,
}

impl NewSessionActions {
	/// Perform the queued actions, feeding into gossip.
	pub fn perform(
		self,
		gossip: &mut dyn crate::GossipService,
		ctx: &mut dyn substrate_network::Context<Block>,
	) {
		for action in self.actions {
			match action {
				NewSessionAction::TargetedMessage(who, message)
					=> gossip.send_message(ctx, &who, message),
				NewSessionAction::Multicast(topic, message)
					=> gossip.multicast(ctx, &topic, message),
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

	/// Note a live attestation session. This must be removed later with
	/// `remove_session`.
	pub(crate) fn note_session(
		&self,
		relay_parent: Hash,
		validation: MessageValidationData,
		lookup_queue_by_root: impl Fn(&Hash) -> Option<Vec<ParachainMessage>>,
	) -> NewSessionActions {
		// add an entry in attestation_view
		// prune any entries from attestation_view which are no longer leaves
		let mut inner = self.inner.inner.write();
		inner.attestation_view.add_session(relay_parent, validation);

		let mut actions = Vec::new();

		{
			let &mut Inner {
				ref chain,
				ref mut attestation_view,
				ref mut message_routing_view,
				..
			} = &mut *inner;

			attestation_view.prune_old_sessions(|parent| match chain.is_known(parent) {
				Some(Known::Leaf) => true,
				_ => false,
			});

			if let Err(e) = message_routing_view.update_leaves(chain, attestation_view.neighbor_info()) {
				warn!("Unable to fully update leaf-state: {:?}", e);
			}
		}


		// send neighbor packets to peers
		inner.multicast_neighbor_packet(
			|who, message| actions.push(NewSessionAction::TargetedMessage(who.clone(), message))
		);

		// feed any new unrouted queues into the propagation pool.
		inner.message_routing_view.sweep_unknown_queues(|topic, queue_root|
			match lookup_queue_by_root(queue_root) {
				Some(messages) => {
					let message = GossipMessage::ParachainMessages(GossipParachainMessages {
						queue_root: *queue_root,
						messages,
					});

					let message = ConsensusMessage {
						data: message.encode(),
						engine_id: POLKADOT_ENGINE_ID,
					};

					actions.push(NewSessionAction::Multicast(*topic, message));

					true
				}
				None => false,
			}
		);

		NewSessionActions { actions }
	}
}

/// The data needed for validating gossip.
#[derive(Default)]
pub(crate) struct MessageValidationData {
	/// The authorities' parachain validation keys at a block.
	pub(crate) authorities: Vec<ValidatorId>,
}

impl MessageValidationData {
	fn check_statement(&self, relay_parent: &Hash, statement: &SignedStatement) -> Result<(), ()> {
		let sender = match self.authorities.get(statement.sender as usize) {
			Some(val) => val,
			None => return Err(()),
		};

		let good = self.authorities.contains(&sender) &&
			::polkadot_validation::check_statement(
				&statement.statement,
				&statement.signature,
				sender.clone(),
				relay_parent,
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
				new_leaves.into_iter().map(attestation_topic).collect()
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
		let neighbor_packet = GossipMessage::Neighbor(VersionedNeighborPacket::V1(NeighborPacket {
			chain_heads: self.attestation_view.neighbor_info().collect(),
		}));

		let message = ConsensusMessage {
			data: neighbor_packet.encode(),
			engine_id: POLKADOT_ENGINE_ID,
		};

		for peer in self.peers.keys() {
			send_neighbor_packet(peer, message.clone())
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

	fn validate(&self, context: &mut dyn ValidatorContext<Block>, sender: &PeerId, mut data: &[u8])
		-> GossipValidationResult<Hash>
	{
		let (res, cost_benefit) = match GossipMessage::decode(&mut data) {
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
					inner.attestation_view.validate_statement(statement, &inner.chain)
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
			// check that topic is one of our live sessions. everything else is expired
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
	use substrate_primitives::sr25519::Signature as Sr25519Signature;
	use polkadot_validation::GenericStatement;

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

		let message = GossipMessage::Neighbor(VersionedNeighborPacket::V1(NeighborPacket {
				chain_heads: vec![hash_a, hash_b],
			})).encode();
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
			relay_parent: hash_a,
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
		validator.inner.write().attestation_view.add_session(hash_a, MessageValidationData::default());
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

		let message = GossipMessage::Neighbor(VersionedNeighborPacket::V1(NeighborPacket {
				chain_heads,
			})).encode();
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

		let message = GossipMessage::Neighbor(VersionedNeighborPacket::V1(NeighborPacket {
				chain_heads: vec![hash_a, hash_b],
			})).encode();
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
			relay_parent: hash_a,
			signed_statement: SignedStatement {
				statement: GenericStatement::Valid(c_hash),
				signature: Sr25519Signature([255u8; 64]).into(),
				sender: 1,
			}
		});
		let encoded = statement.encode();
		validator.inner.write().attestation_view.add_session(hash_a, MessageValidationData::default());

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
			.note_aware_in_session(&hash_a, c_hash);
		{
			let mut message_allowed = validator.message_allowed();
			assert!(message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded[..]));
		}
	}
}
