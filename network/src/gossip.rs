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
	ValidatorContext, MessageIntent,
};
use polkadot_validation::SignedStatement;
use polkadot_primitives::{Block, Hash, SessionKey, parachain::ValidatorIndex};
use codec::Decode;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::NetworkService;

/// The engine ID of the polkadot attestation system.
pub const POLKADOT_ENGINE_ID: sr_primitives::ConsensusEngineId = [b'd', b'o', b't', b'1'];

// arbitrary; in practice this should not be more than 2.
const MAX_CHAIN_HEADS: usize = 5;

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
}

/// A gossip message.
#[derive(Encode, Decode, Clone)]
pub(crate) enum GossipMessage {
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

/// A gossip message containing a statement.
#[derive(Encode, Decode, Clone)]
pub(crate) struct GossipStatement {
	/// The relay chain parent hash.
	pub(crate) relay_parent: Hash,
	/// The signed statement being gossipped.
	pub(crate) statement: SignedStatement,
}

/// A versioned neighbor message.
#[derive(Encode, Decode, Clone)]
pub enum VersionedNeighborPacket {
	#[codec(index = "1")]
	V1(NeighborPacket),
}

impl VersionedNeighborPacket {
	fn into_inner(self) -> NeighborPacket {
		match self {
			VersionedNeighborPacket::V1(packet) => packet,
		}
	}
}

/// Contains information on which chain heads the peer is
/// accepting messages for.
#[derive(Encode, Decode, Clone)]
pub struct NeighborPacket {
	chain_heads: Vec<Hash>,
}

/// whether a block is known.
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
	service: &NetworkService,
	oracle: O,
) -> RegisteredMessageValidator {
	let validator = Arc::new(MessageValidator {
		inner: RwLock::new(Inner {
			peers: HashMap::new(),
			our_live_sessions: HashMap::new(),
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
	inner: Arc<MessageValidator<KnownOracle>>,
}

impl RegisteredMessageValidator {
	#[cfg(test)]
	pub(crate) fn new_test<O: KnownOracle + 'static>(oracle: O) -> Self {
		let validator = Arc::new(MessageValidator {
			inner: RwLock::new(Inner {
				peers: HashMap::new(),
				our_live_sessions: HashMap::new(),
				oracle,
			})
		});

		RegisteredMessageValidator { inner: validator as _ }
	}

	/// Note a live attestation session. This must be removed later with
	/// `remove_session`.
	pub(crate) fn note_session(&self, relay_parent: Hash, validation: MessageValidationData) {
		unimplemented!()
	}

	/// Remove a live attestation session when it is no longer live.
	pub(crate) fn remove_session(&self, relay_parent: &Hash) {
		unimplemented!()
	}
}

/// The data needed for validating gossip.
pub(crate) struct MessageValidationData {
	/// The authorities at a block.
	pub(crate) authorities: Vec<SessionKey>,
	/// Mapping from validator index to `SessionKey`.
	pub(crate) index_mapping: HashMap<ValidatorIndex, SessionKey>,
}

impl MessageValidationData {
	fn check_statement(&self, relay_parent: &Hash, statement: &SignedStatement) -> bool {
		let sender = match self.index_mapping.get(&statement.sender) {
			Some(val) => val,
			None => return false,
		};

		self.authorities.contains(&sender) &&
			::polkadot_validation::check_statement(
				&statement.statement,
				&statement.signature,
				sender.clone(),
				relay_parent,
			)
	}
}

// knowledge about attestations on a single parent-hash.
struct Knowledge {
	candidates: HashMap<Hash, Vec<SessionKey>>,
}

impl Knowledge {
	// whether the peer is aware of a candidate with given hash.
	fn is_aware_of(&self, candidate_hash: &Hash) -> bool {
		self.candidates.contains_key(candidate_hash)
	}

	// whether the peer is aware of a candidate with given hash from a specific author.
	fn is_aware_of_from(&self, candidate_hash: &Hash, author_key: &SessionKey) -> bool {
		self.candidates.get(candidate_hash)
			.and_then(|f| f.iter().position(|k| k == author_key)).is_some()
	}
}

struct PeerData {
	live: HashMap<Hash, Knowledge>,
}

impl PeerData {
	fn knowledge_at(&self, parent_hash: &Hash) -> Option<&Knowledge> {
		self.live.get(parent_hash)
	}

	fn believes_live(&self, parent_hash: &Hash) -> bool {
		self.live.contains_key(&parent_hash)
	}
}

struct Inner<O: ?Sized> {
	peers: HashMap<PeerId, PeerData>,
	our_live_sessions: HashMap<Hash, MessageValidationData>,
	oracle: O,
}

impl<O: ?Sized + KnownOracle> Inner<O> {
	fn validate_statement(&mut self, sender: &PeerId, statement: GossipStatement) {

	}
}

/// An unregistered message validator. Register this with `register_validator`.
pub struct MessageValidator<O: ?Sized> {
	inner: RwLock<Inner<O>>,
}

impl<O: KnownOracle + ?Sized> MessageValidator<O> {
	fn report(&self, _who: &PeerId, cost_benefit: i32) {

	}
}

impl<O: KnownOracle + ?Sized> network_gossip::Validator<Block> for MessageValidator<O> {
	fn new_peer(&self, _context: &mut ValidatorContext<Block>, who: &PeerId, _roles: Roles) {
		unimplemented!()
	}

	fn peer_disconnected(&self, _context: &mut ValidatorContext<Block>, who: &PeerId) {
		unimplemented!()
	}

	fn validate(&self, context: &mut ValidatorContext<Block>, _sender: &PeerId, mut data: &[u8])
		-> GossipValidationResult<Hash>
	{
		unimplemented!()
	}

	fn message_expired<'a>(&'a self) -> Box<FnMut(Hash, &[u8]) -> bool + 'a> {
		Box::new(move |_topic, _data| false)
	}

	fn message_allowed<'a>(&'a self) -> Box<FnMut(&PeerId, MessageIntent, &Hash, &[u8]) -> bool + 'a> {
		Box::new(move |_who, _intent, _topic, _data| true)
	}
}
