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

use sp_runtime::traits::{Hash as HashT};
pub use sp_runtime::traits::BlakeTwo256;
use sp_blockchain::Error as ClientError;
pub use sc_network::{config::Roles};
pub use sc_network::{PeerId, ReputationChange, NetworkService};
pub use sc_network_gossip::{
	ValidatorContext, GossipEngine,
};
pub use sc_network_gossip::{ValidationResult as GossipValidationResult, Validator, MessageIntent};
use polkadot_statement_table::{SignedStatement};
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::{
    ParachainHost, ValidatorId, ErasureChunk as PrimitiveChunk, SigningContext,
    Statement as PrimitiveStatement, ValidatorSignature,
};
pub use polkadot_erasure_coding::{self as erasure};
use codec::{Decode, Encode};
use sp_api::ProvideRuntimeApi;

use std::collections::HashMap;
use std::sync::Arc;
use std::pin::Pin;
use std::task::{Poll, Context as PollContext};
use log::debug;

use arrayvec::ArrayVec;
use futures::prelude::*;
use parking_lot::{Mutex, RwLock};

pub use attestation::{View as AttestationView};
pub use attestation::PeerData as AttestationPeerData;

mod attestation;
use sc_network_gossip::TopicNotification;

/// The engine ID of the polkadot attestation system.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot1";
pub const POLKADOT_PROTOCOL_NAME: &[u8] = b"/polkadot/legacy/1";

// arbitrary; in practice this should not be more than 2.
pub const MAX_CHAIN_HEADS: usize = 5;

/// Type alias for a bounded vector of leaves.
pub type LeavesVec = ArrayVec<[Hash; MAX_CHAIN_HEADS]>;

pub mod benefit {
	use sc_network::ReputationChange as Rep;
	/// When a peer sends us a previously-unknown candidate statement.
	pub const NEW_CANDIDATE: Rep = Rep::new(100, "Polkadot: New candidate");
	/// When a peer sends us a previously-unknown attestation.
	pub const NEW_ATTESTATION: Rep = Rep::new(50, "Polkadot: New attestation");
	/// When a peer sends us a previously-unknown erasure chunk.
	pub const NEW_ERASURE_CHUNK: Rep = Rep::new(10, "Polkadot: New erasure chunk");
}

pub mod cost {
	use sc_network::ReputationChange as Rep;
	/// No cost. This will not be reported.
	pub const NONE: Rep = Rep::new(0, "");
	/// A peer sent us an attestation and we don't know the candidate.
	pub const ATTESTATION_NO_CANDIDATE: Rep = Rep::new(-100, "Polkadot: No candidate");
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

/// A packet of messages from one parachain to another.
///
/// These are all the messages posted from one parachain to another during the
/// execution of a single parachain block. Since this parachain block may have been
/// included in many forks of the relay chain, there is no relay-chain leaf parameter.
#[derive(Encode, Decode, Clone, PartialEq)]
pub struct GossipParachainMessages {
	/// The root of the message queue.
	pub queue_root: Hash,
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
	pub chain_heads: Vec<Hash>,
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
pub fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

#[derive(PartialEq)]
pub enum NewLeafAction {
	// (who, message)
	TargetedMessage(PeerId, GossipMessage),
}

/// Actions to take after noting a new block-DAG leaf.
///
/// This should be consumed by passing a consensus-gossip handle to `perform`.
#[must_use = "New chain-head gossip actions must be performed"]
pub struct NewLeafActions {
	pub actions: Vec<NewLeafAction>,
}

impl NewLeafActions {
	pub fn new() -> Self {
		NewLeafActions { actions: Vec::new() }
	}

	/// Perform the queued actions, feeding into gossip.
	pub fn perform(
		self,
		gossip: &dyn GossipService,
	) {
		for action in self.actions {
			match action {
				NewLeafAction::TargetedMessage(who, message)
					=> gossip.send_message(who, message),
			}
		}
	}
}

/// Compute gossip topic for the erasure chunk messages given the hash of the
/// candidate they correspond to.
pub fn erasure_coding_topic(candidate_hash: &Hash) -> Hash {
	let mut v = candidate_hash.as_ref().to_vec();
	v.extend(b"erasure_chunks");

	BlakeTwo256::hash(&v[..])
}


/// The data needed for validating gossip messages.
#[derive(Default)]
pub struct MessageValidationData {
	/// The authorities' parachain validation keys at a block.
	pub authorities: Vec<ValidatorId>,
	/// The signing context.
	pub signing_context: SigningContext,
}

impl MessageValidationData {
	// check a statement's signature.
	fn check_statement(&self, statement: &SignedStatement) -> Result<(), ()> {
		let sender = match self.authorities.get(statement.sender as usize) {
			Some(val) => val,
			None => return Err(()),
		};

		let good = self.authorities.contains(&sender) &&
			check_statement(
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

use polkadot_statement_table::Statement;

/// Check signature on table statement.
pub fn check_statement(
	statement: &Statement,
	signature: &ValidatorSignature,
	signer: ValidatorId,
	signing_context: &SigningContext,
) -> bool {
	use sp_runtime::traits::AppVerify;

	let mut encoded = PrimitiveStatement::from(statement).encode();
	encoded.extend(signing_context.encode());

	signature.verify(&encoded[..], &signer)
}

#[derive(Default)]
pub struct PeerData {
	pub attestation: AttestationPeerData,
}

/// Basic gossip functionality that a network has to fulfill.
pub trait GossipService {
	/// Get a stream of gossip messages for a given hash.
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream;

	/// Gossip a message on given topic.
	fn gossip_message(&self, topic: Hash, message: GossipMessage);

	/// Send a message to a specific peer we're connected to.
	fn send_message(&self, who: PeerId, message: GossipMessage);
}

/// A stream of gossip messages and an optional sender for a topic.
pub struct GossipMessageStream {
	topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>,
}

impl GossipMessageStream {
	/// Create a new instance with the given topic stream.
	pub fn new(topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>) -> Self {
		Self {
			topic_stream,
		}
	}
}

impl Stream for GossipMessageStream {
	type Item = (GossipMessage, Option<PeerId>);

	fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext) -> Poll<Option<Self::Item>> {
		let this = Pin::into_inner(self);

		loop {
			let msg = match Pin::new(&mut this.topic_stream).poll_next(cx) {
				Poll::Ready(Some(msg)) => msg,
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Pending => return Poll::Pending,
			};

			debug!(target: "validation", "Processing statement for live validation leaf-work");
			if let Ok(gmsg) = GossipMessage::decode(&mut &msg.message[..]) {
				return Poll::Ready(Some((gmsg, msg.sender)))
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sc_network_gossip::Validator as ValidatorT;
	use std::sync::mpsc;
	use parking_lot::Mutex;
	use polkadot_primitives::parachain::AbridgedCandidateReceipt;
	use sp_core::sr25519::Signature as Sr25519Signature;
    pub use polkadot_statement_table::generic::Statement as GenericStatement;

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
	fn message_allowed() {
		let (tx, _rx) = mpsc::channel();
		let tx = Mutex::new(tx);
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
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
		let report_handle = Box::new(move |peer: &PeerId, cb: ReputationChange| tx.lock().send((peer.clone(), cb)).unwrap());
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
			.note_aware_under_leaf(&hash_a, c_hash);
		{
			let mut message_allowed = validator.message_allowed();
			assert!(message_allowed(&peer_a, MessageIntent::Broadcast, &topic_a, &encoded[..]));
		}
	}
}
