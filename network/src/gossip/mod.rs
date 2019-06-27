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

use substrate_network::{config::Roles, PeerId};
use substrate_network::consensus_gossip::{
	self as network_gossip, ValidationResult as GossipValidationResult,
	ValidatorContext, MessageIntent, ConsensusMessage,
};
use polkadot_validation::SignedStatement;
use polkadot_primitives::{Block, Hash, SessionKey, parachain::ValidatorIndex};
use parity_codec::{Decode, Encode};

use std::collections::HashMap;
use std::sync::Arc;

use arrayvec::ArrayVec;
use parking_lot::RwLock;

use super::NetworkService;
use crate::router::attestation_topic;

use attestation::{View as AttestationView, PeerData as AttestationPeerData};

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

/// An oracle for known blocks.
pub trait KnownOracle: Send + Sync {
	/// whether a block is known. If it's not, returns `None`.
	fn is_known(&self, block_hash: &Hash) -> Option<Known>;
}

impl<F> KnownOracle for F where F: Fn(&Hash) -> Option<Known> + Send + Sync {
	fn is_known(&self, block_hash: &Hash) -> Option<Known> {
		(self)(block_hash)
	}
}

/// Register a gossip validator on the network service.
///
/// This returns a `RegisteredMessageValidator`
// NOTE: since RegisteredMessageValidator is meant to be a type-safe proof
// that we've actually done the registration, this should be the only way
// to construct it outside of tests.
pub fn register_validator<O: KnownOracle + 'static>(
	service: Arc<NetworkService>,
	oracle: O,
) -> RegisteredMessageValidator {
	let s = service.clone();
	let report_handle = Box::new(move |peer: &PeerId, cost_benefit| {
		s.report_peer(peer.clone(), cost_benefit);
	});
	let validator = Arc::new(MessageValidator {
		report_handle,
		inner: RwLock::new(Inner {
			peers: HashMap::new(),
			attestation_view: Default::default(),
			oracle,
		})
	});

	let gossip_side = validator.clone();
	service.with_gossip(|gossip, ctx|
		gossip.register_validator(ctx, POLKADOT_ENGINE_ID, gossip_side)
	);

	RegisteredMessageValidator { inner: validator as _ }
}

/// A registered message validator.
///
/// Create this using `register_validator`.
#[derive(Clone)]
pub struct RegisteredMessageValidator {
	inner: Arc<MessageValidator<dyn KnownOracle>>,
}

impl RegisteredMessageValidator {
	#[cfg(test)]
	pub(crate) fn new_test<O: KnownOracle + 'static>(
		oracle: O,
		report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	) -> Self {
		let validator = Arc::new(MessageValidator::new_test(oracle, report_handle));

		RegisteredMessageValidator { inner: validator as _ }
	}

	/// Note a live attestation session. This must be removed later with
	/// `remove_session`.
	pub(crate) fn note_session<F: FnMut(&PeerId, ConsensusMessage)>(
		&self,
		relay_parent: Hash,
		validation: MessageValidationData,
		send_neighbor_packet: F,
	) {
		// add an entry in attestation_view
		// prune any entries from attestation_view which are no longer leaves
		let mut inner = self.inner.inner.write();
		inner.attestation_view.add_session(relay_parent, validation);
		{

			let &mut Inner { ref oracle, ref mut attestation_view, .. } = &mut *inner;
			attestation_view.prune_old_sessions(|parent| match oracle.is_known(parent) {
				Some(Known::Leaf) => true,
				_ => false,
			});
		}

		// send neighbor packets to peers
		inner.multicast_neighbor_packet(send_neighbor_packet);
	}
}

/// The data needed for validating gossip.
#[derive(Default)]
pub(crate) struct MessageValidationData {
	/// The authorities at a block.
	pub(crate) authorities: Vec<SessionKey>,
	/// Mapping from validator index to `SessionKey`.
	pub(crate) index_mapping: HashMap<ValidatorIndex, SessionKey>,
}

impl MessageValidationData {
	fn check_statement(&self, relay_parent: &Hash, statement: &SignedStatement) -> Result<(), ()> {
		let sender = match self.index_mapping.get(&statement.sender) {
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

struct Inner<O: ?Sized> {
	peers: HashMap<PeerId, PeerData>,
	attestation_view: AttestationView,
	oracle: O,
}

impl<O: ?Sized + KnownOracle> Inner<O> {
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
			chain_heads: self.attestation_view.neighbor_info()
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
pub struct MessageValidator<O: ?Sized> {
	report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	inner: RwLock<Inner<O>>,
}

impl<O: KnownOracle + ?Sized> MessageValidator<O> {
	#[cfg(test)]
	fn new_test(
		oracle: O,
		report_handle: Box<dyn Fn(&PeerId, i32) + Send + Sync>,
	) -> Self where O: Sized{
		MessageValidator {
			report_handle,
			inner: RwLock::new(Inner {
				peers: HashMap::new(),
				attestation_view: Default::default(),
				oracle,
			})
		}
	}

	fn report(&self, who: &PeerId, cost_benefit: i32) {
		(self.report_handle)(who, cost_benefit)
	}
}

impl<O: KnownOracle + ?Sized> network_gossip::Validator<Block> for MessageValidator<O> {
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
			None => (GossipValidationResult::Discard, cost::MALFORMED_MESSAGE),
			Some(GossipMessage::Neighbor(VersionedNeighborPacket::V1(packet))) => {
				let (res, cb, topics) = self.inner.write().validate_neighbor_packet(sender, packet);
				for new_topic in topics {
					context.send_topic(sender, new_topic, false);
				}
				(res, cb)
			}
			Some(GossipMessage::Statement(statement)) => {
				let (res, cb) = {
					let mut inner = self.inner.write();
					let inner = &mut *inner;
					inner.attestation_view.validate_statement(statement, &inner.oracle)
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
			!inner.attestation_view.is_topic_live(&topic)
		})
	}

	fn message_allowed<'a>(&'a self) -> Box<dyn FnMut(&PeerId, MessageIntent, &Hash, &[u8]) -> bool + 'a> {
		let mut inner = self.inner.write();
		Box::new(move |who, intent, topic, data| {
			let &mut Inner { ref mut peers, ref mut attestation_view, .. } = &mut *inner;

			match intent {
				MessageIntent::PeriodicRebroadcast => return false,
				_ => {},
			}

			let attestation_head = attestation_view.topic_block(topic).map(|x| x.clone());
			let peer_knowledge = peers.get_mut(who)
				.and_then(move |p| attestation_head.map(|r| (p, r)))
				.and_then(|(p, r)| p.attestation.knowledge_at_mut(&r).map(|k| (k, r)));

			match GossipMessage::decode(&mut &data[..]) {
				Some(GossipMessage::Statement(ref statement)) =>
					// to allow statements, we need peer knowledge.
					peer_knowledge.map_or(false, |(knowledge, attestation_head)| {
						attestation_view.statement_allowed(
							statement,
							&attestation_head,
							knowledge,
						)
					}),
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
	use substrate_primitives::ed25519::Signature as Ed25519Signature;
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
		let known_map = HashMap::<Hash, Known>::new();
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			move |hash: &Hash| known_map.get(hash).map(|x| x.clone()),
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
				signature: Ed25519Signature([255u8; 64]),
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
		let known_map = HashMap::<Hash, Known>::new();
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			move |hash: &Hash| known_map.get(hash).map(|x| x.clone()),
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
		let known_map = HashMap::<Hash, Known>::new();
		let report_handle = Box::new(move |peer: &PeerId, cb: i32| tx.lock().send((peer.clone(), cb)).unwrap());
		let validator = MessageValidator::new_test(
			move |hash: &Hash| known_map.get(hash).map(|x| x.clone()),
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
				signature: Ed25519Signature([255u8; 64]),
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
