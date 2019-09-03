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

//! Gossip messages and structures for dealing with attestations (statements of
//! validity of invalidity on parachain candidates).
//!
//! This follows the same principles as other gossip modules (see parent
//! documentation for more details) by being aware of our current chain
//! heads and accepting only information relative to them. Attestations are localized to
//! relay chain head, so this is easily doable.
//!
//! This module also provides a filter, so we can only broadcast messages to
//! peers that are relevant to chain heads they have advertised.
//!
//! Furthermore, since attestations are bottlenecked by the `Candidate` statement,
//! we only accept attestations which are themselves `Candidate` messages, or reference
//! a `Candidate` we are aware of. Otherwise, it is possible we could be forced to
//! consider an infinite amount of attestations produced by a misbehaving validator.

use substrate_network::consensus_gossip::{ValidationResult as GossipValidationResult};
use polkadot_validation::GenericStatement;
use polkadot_primitives::Hash;

use std::collections::{HashMap, HashSet};

use log::warn;
use crate::router::attestation_topic;

use super::{cost, benefit, MAX_CHAIN_HEADS, LeavesVec, ChainContext, Known, MessageValidationData, GossipStatement};

// knowledge about attestations on a single parent-hash.
#[derive(Default)]
pub(super) struct Knowledge {
	candidates: HashSet<Hash>,
}

impl Knowledge {
	// whether the peer is aware of a candidate with given hash.
	fn is_aware_of(&self, candidate_hash: &Hash) -> bool {
		self.candidates.contains(candidate_hash)
	}

	// note that the peer is aware of a candidate with given hash. this should
	// be done after observing an incoming candidate message via gossip.
	fn note_aware(&mut self, candidate_hash: Hash) {
		self.candidates.insert(candidate_hash);
	}
}

#[derive(Default)]
pub(super) struct PeerData {
	live: HashMap<Hash, Knowledge>,
}

impl PeerData {
	/// Update leaves, returning a list of which leaves are new.
	pub(super) fn update_leaves(&mut self, leaves: &LeavesVec) -> LeavesVec {
		let mut new = LeavesVec::new();
		self.live.retain(|k, _| leaves.contains(k));
		for &leaf in leaves {
			self.live.entry(leaf).or_insert_with(|| {
				new.push(leaf);
				Default::default()
			});
		}

		new
	}

	#[cfg(test)]
	pub(super) fn note_aware_under_leaf(&mut self, relay_chain_leaf: &Hash, candidate_hash: Hash) {
		if let Some(knowledge) = self.live.get_mut(relay_chain_leaf) {
			knowledge.note_aware(candidate_hash);
		}
	}

	pub(super) fn knowledge_at_mut(&mut self, parent_hash: &Hash) -> Option<&mut Knowledge> {
		self.live.get_mut(parent_hash)
	}

	/// Get an iterator over all live leaves of this peer.
	pub(super) fn leaves(&self) -> impl Iterator<Item = &Hash> {
		self.live.keys()
	}
}

/// An impartial view of what topics and data are valid based on attestation session data.
pub(super) struct View {
	leaf_work: Vec<(Hash, LeafView)>, // hashes of the best DAG-leaves paired with validation data.
	topics: HashMap<Hash, Hash>, // maps topic hashes to block hashes.
}

impl Default for View {
	fn default() -> Self {
		View {
			leaf_work: Vec::with_capacity(MAX_CHAIN_HEADS),
			topics: Default::default(),
		}
	}
}

impl View {
	fn leaf_view(&self, relay_chain_leaf: &Hash) -> Option<&LeafView> {
		self.leaf_work.iter()
			.find_map(|&(ref h, ref leaf)| if h == relay_chain_leaf { Some(leaf) } else { None } )
	}

	fn leaf_view_mut(&mut self, relay_chain_leaf: &Hash) -> Option<&mut LeafView> {
		self.leaf_work.iter_mut()
			.find_map(|&mut (ref h, ref mut leaf)| if h == relay_chain_leaf { Some(leaf) } else { None } )
	}

	/// Get our leaves-set. Guaranteed to have length <= MAX_CHAIN_HEADS.
	pub(super) fn neighbor_info<'a>(&'a self) -> impl Iterator<Item=Hash> + 'a + Clone {
		self.leaf_work.iter().take(MAX_CHAIN_HEADS).map(|(p, _)| p.clone())
	}

	/// Note new leaf in our local view and validation data necessary to check signatures
	/// of statements issued under this leaf.
	///
	/// This will be pruned later on a call to `prune_old_leaves`, when this leaf
	/// is not a leaf anymore.
	pub(super) fn new_local_leaf(&mut self, relay_chain_leaf: Hash, validation_data: MessageValidationData) {
		self.leaf_work.push((
			relay_chain_leaf,
			LeafView {
				validation_data,
				knowledge: Default::default(),
			},
		));
		self.topics.insert(attestation_topic(relay_chain_leaf), relay_chain_leaf);
	}

	/// Prune old leaf-work that fails the leaf predicate.
	pub(super) fn prune_old_leaves<F: Fn(&Hash) -> bool>(&mut self, is_leaf: F) {
		let leaf_work = &mut self.leaf_work;
		leaf_work.retain(|&(ref relay_chain_leaf, _)| is_leaf(relay_chain_leaf));
		self.topics.retain(|_, v| leaf_work.iter().find(|(p, _)| p == v).is_some());
	}

	/// Whether a message topic is considered live relative to our view. non-live
	/// topics do not pertain to our perceived leaves, and are uninteresting to us.
	pub(super) fn is_topic_live(&self, topic: &Hash) -> bool {
		self.topics.contains_key(topic)
	}

	/// The relay-chain block hash corresponding to a topic.
	pub(super) fn topic_block(&self, topic: &Hash) -> Option<&Hash> {
		self.topics.get(topic)
	}


	/// Validate the signature on an attestation statement of some kind. Should be done before
	/// any repropagation of that statement.
	pub(super) fn validate_statement_signature<C: ChainContext + ?Sized>(
		&mut self,
		message: GossipStatement,
		chain: &C,
	)
		-> (GossipValidationResult<Hash>, i32)
	{
		// message must reference one of our chain heads and
		// if message is not a `Candidate` we should have the candidate available
		// in `attestation_view`.
		match self.leaf_view(&message.relay_chain_leaf) {
			None => {
				let cost = match chain.is_known(&message.relay_chain_leaf) {
					Some(Known::Leaf) => {
						warn!(
							target: "network",
							"Leaf block {} not considered live for attestation",
							message.relay_chain_leaf,
						);

						0
					}
					Some(Known::Old) => cost::PAST_MESSAGE,
					_ => cost::FUTURE_MESSAGE,
				};

				(GossipValidationResult::Discard, cost)
			}
			Some(view) => {
				// first check that we are capable of receiving this message
				// in a DoS-proof manner.
				let benefit = match message.signed_statement.statement {
					GenericStatement::Candidate(_) => benefit::NEW_CANDIDATE,
					GenericStatement::Valid(ref h) | GenericStatement::Invalid(ref h) => {
						if !view.knowledge.is_aware_of(h) {
							let cost = cost::ATTESTATION_NO_CANDIDATE;
							return (GossipValidationResult::Discard, cost);
						}

						benefit::NEW_ATTESTATION
					}
				};

				// validate signature.
				let res = view.validation_data.check_statement(
					&message.relay_chain_leaf,
					&message.signed_statement,
				);

				match res {
					Ok(()) => {
						let topic = attestation_topic(message.relay_chain_leaf);
						(GossipValidationResult::ProcessAndKeep(topic), benefit)
					}
					Err(()) => (GossipValidationResult::Discard, cost::BAD_SIGNATURE),
				}
			}
		}
	}

	/// whether it's allowed to send a statement to a peer with given knowledge
	/// about the relay parent the statement refers to.
	pub(super) fn statement_allowed(
		&mut self,
		statement: &GossipStatement,
		relay_chain_leaf: &Hash,
		peer_knowledge: &mut Knowledge,
	) -> bool {
		let signed = &statement.signed_statement;

		match signed.statement {
			GenericStatement::Valid(ref h) | GenericStatement::Invalid(ref h) => {
				// `valid` and `invalid` statements can only be propagated after
				// a candidate message is known by that peer.
				peer_knowledge.is_aware_of(h)
			}
			GenericStatement::Candidate(ref c) => {
				// if we are sending a `Candidate` message we should make sure that
				// attestation_view and their_view reflects that we know about the candidate.
				let hash = c.hash();
				peer_knowledge.note_aware(hash);
				if let Some(attestation_view) = self.leaf_view_mut(&relay_chain_leaf) {
					attestation_view.knowledge.note_aware(hash);
				}

				// at this point, the peer hasn't seen the message or the candidate
				// and has knowledge of the relevant relay-chain parent.
				true
			}
		}
	}
}

struct LeafView {
	validation_data: MessageValidationData,
	knowledge: Knowledge,
}
