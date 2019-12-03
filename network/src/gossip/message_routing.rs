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

//! Data structures and synchronous logic for ICMP message gossip.
//!
//! The parent-module documentation describes some rationale of the general
//! gossip protocol design.
//!
//! The ICMP message-routing gossip works according to those rationale.
//!
//! In this protocol, we perform work under 4 conditions:
//! ### 1. Upon observation of a new leaf in the block-DAG.
//!
//! We first communicate the best leaves to our neighbors in the gossip graph
//! by the means of a neighbor packet. Then, we query to discover the trie roots
//! of all un-routed message queues from the perspective of each of those leaves.
//!
//! For any trie root in the unrouted set for the new leaf, if we have the corresponding
//! queue, we send it to any peers with the new leaf in their latest advertised set.
//!
//! Which parachain those messages go to and from is unimportant, because this is
//! an everybody-sees-everything style protocol. The only important property is "liveness":
//! that the queue root is un-routed at one of the leaves we perceive to be at the head
//! of the block-DAG.
//!
//! In Substrate gossip, every message is associated with a topic. Typically,
//! many messages are grouped under a single topic. In this gossip system, each queue
//! gets its own topic, which is based on the root hash of the queue. This is because
//! many different chain leaves may have the same queue as un-routed, so it's better than
//! attempting to group message packets by the leaf they appear unrouted at.
//!
//! ### 2. Upon a neighbor packet from a peer.
//!
//! The neighbor packet from a peer should contain perceived chain heads of that peer.
//! If there is any overlap between our perceived chain heads and theirs, we send
//! them any known, un-routed message queue from either set.
//!
//! ### 3. Upon receiving a message queue from a peer.
//!
//! If the message queue is in the un-routed set of one of the latest leaves we've updated to,
//! we accept it and relay to any peers who need that queue as well.
//!
//! If not, we report the peer to the peer-set manager for sending us bad data.
//!
//! ### 4. Periodic Pruning
//!
//! We prune messages that are not un-routed from the view of any leaf and cease
//! to attempt to send them to any peer.

use sp_runtime::traits::{BlakeTwo256, Hash as HashT};
use polkadot_primitives::Hash;
use std::collections::{HashMap, HashSet};
use sp_blockchain::Error as ClientError;
use super::{MAX_CHAIN_HEADS, GossipValidationResult, LeavesVec, ChainContext};

/// Construct a topic for a message queue root deterministically.
pub fn queue_topic(queue_root: Hash) -> Hash {
	let mut v = queue_root.as_ref().to_vec();
	v.extend(b"message_queue");

	BlakeTwo256::hash(&v[..])
}

/// A view of which queue roots are current for a given set of leaves.
#[derive(Default)]
pub struct View {
	leaves: LeavesVec,
	leaf_topics: HashMap<Hash, HashSet<Hash>>, // leaf_hash -> { topics }
	expected_queues: HashMap<Hash, (Hash, bool)>, // topic -> (queue-root, known)
}

impl View {
	/// Update the set of current leaves. This is called when we perceive a new bset leaf-set.
	pub fn update_leaves<T: ChainContext + ?Sized, I>(&mut self, context: &T, new_leaves: I)
		-> Result<(), ClientError>
		where I: Iterator<Item=Hash>
	{
		let new_leaves = new_leaves.take(MAX_CHAIN_HEADS);
		let old_leaves = std::mem::replace(&mut self.leaves, new_leaves.collect());

		let expected_queues = &mut self.expected_queues;
		let leaves = &self.leaves;
		self.leaf_topics.retain(|l, topics| {
			if leaves.contains(l) { return true }

			// prune out all data about old leaves we don't follow anymore.
			for topic in topics.iter() {
				expected_queues.remove(topic);
			}
			false
		});

		let mut res = Ok(());

		// add in new data about fresh leaves.
		for new_leaf in &self.leaves {
			if old_leaves.contains(new_leaf) { continue }

			let mut this_leaf_topics = HashSet::new();

			let r = context.leaf_unrouted_roots(new_leaf, &mut |&queue_root| {
				let topic = queue_topic(queue_root);
				this_leaf_topics.insert(topic);
				expected_queues.entry(topic).or_insert((queue_root, false));
			});

			if r.is_err() {
				if let Err(e) = res {
					log::debug!(target: "message_routing", "Ignored duplicate error {}", e)
				};
				res = r;
			}

			self.leaf_topics.insert(*new_leaf, this_leaf_topics);
		}

		res
	}

	/// Validate an incoming message queue against this view. If it is accepted
	/// by our view of un-routed message queues, we will keep and re-propagate.
	pub fn validate_queue_and_note_known(&mut self, messages: &super::GossipParachainMessages)
		-> (GossipValidationResult<Hash>, sc_network::ReputationChange)
	{
		let ostensible_topic = queue_topic(messages.queue_root);
		match self.expected_queues.get_mut(&ostensible_topic) {
			None => (GossipValidationResult::Discard, super::cost::UNNEEDED_ICMP_MESSAGES),
			Some(&mut (_, ref mut known)) => {
				if !messages.queue_root_is_correct() {
					(
						GossipValidationResult::Discard,
						super::cost::icmp_messages_root_mismatch(messages.messages.len()),
					)
				} else {
					*known = true;
					(
						GossipValidationResult::ProcessAndKeep(ostensible_topic),
						super::benefit::NEW_ICMP_MESSAGES,
					)
				}
			}
		}
	}

	/// Whether a message with given topic is live.
	pub fn is_topic_live(&self, topic: &Hash) -> bool {
		self.expected_queues.get(topic).is_some()
	}

	/// Whether a message is allowed under the intersection of the given leaf-set
	/// and our own.
	pub fn allowed_intersecting(&self, other_leaves: &LeavesVec, topic: &Hash) -> bool {
		for i in other_leaves {
			for j in &self.leaves {
				if i == j {
					let leaf_topics = self.leaf_topics.get(i)
						.expect("leaf_topics are mutated only in update_leaves; \
							we have an entry for each item in self.leaves; \
							i is in self.leaves; qed");

					if leaf_topics.contains(topic) {
						return true;
					}
				}
			}
		}

		false
	}

	/// Get topics of all message queues a peer is interested in - this is useful
	/// when a peer has informed us of their new best leaves.
	pub fn intersection_topics(&self, other_leaves: &LeavesVec) -> impl Iterator<Item=Hash> {
		let deduplicated = other_leaves.iter()
			.filter_map(|l| self.leaf_topics.get(l))
			.flat_map(|topics| topics.iter().cloned())
			.collect::<HashSet<_>>();

		deduplicated.into_iter()
	}

	/// Iterate over all live message queues for which the data is marked as not locally known,
	/// calling a closure with `(topic, root)`. The closure will return whether the queue data is
	/// unknown.
	///
	/// This is called when we should send un-routed message queues that we are
	/// newly aware of to peers - as in when we update our leaves.
	pub fn sweep_unknown_queues(&mut self, mut check_known: impl FnMut(&Hash, &Hash) -> bool) {
		for (topic, &mut (ref queue_root, ref mut known)) in self.expected_queues.iter_mut() {
			if !*known {
				*known = check_known(topic, queue_root)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tests::TestChainContext;
	use crate::gossip::{Known, GossipParachainMessages};
	use polkadot_primitives::parachain::Message as ParachainMessage;

	fn hash(x: u8) -> Hash {
		[x; 32].into()
	}

	fn message_queue(from: u8, to: u8) -> Option<[[u8; 2]; 1]> {
		if from == to {
			None
		} else {
			Some([[from, to]])
		}
	}

	fn message_queue_root(from: u8, to: u8) -> Option<Hash> {
		message_queue(from, to).map(
			|q| polkadot_validation::message_queue_root(q.iter())
		)
	}

	// check that our view has all of the roots of the message queues
	// emitted in the heads identified in `our_heads`, and none of the others.
	fn check_roots(view: &mut View, our_heads: &[u8], n_heads: u8) -> bool {
		for i in 0..n_heads {
			for j in 0..n_heads {
				if let Some(messages) = message_queue(i, j) {
					let queue_root = message_queue_root(i, j).unwrap();
					let messages = GossipParachainMessages {
						queue_root,
						messages: messages.iter().map(|m| ParachainMessage(m.to_vec())).collect(),
					};

					let had_queue = match view.validate_queue_and_note_known(&messages).0 {
						GossipValidationResult::ProcessAndKeep(topic) => topic == queue_topic(queue_root),
						_ => false,
					};

					if our_heads.contains(&i) != had_queue {
						return false
					}
				}
			}
		}

		true
	}

	#[test]
	fn update_leaves_none_in_common() {
		let mut ctx = TestChainContext::default();
		let n_heads = 5;

		for i in 0..n_heads {
			ctx.known_map.insert(hash(i as u8), Known::Leaf);

			let messages_out: Vec<_> = (0..n_heads).filter_map(|j| message_queue_root(i, j)).collect();

			if !messages_out.is_empty() {
				ctx.ingress_roots.insert(hash(i as u8), messages_out);
			}
		}

		// initialize the view with 2 leaves.

		let mut view = View::default();
		view.update_leaves(
			&ctx,
			[hash(0), hash(1)].iter().cloned(),
		).unwrap();

		// we should have all queue roots that were
		// un-routed from the perspective of those 2
		// leaves and no others.

		assert!(check_roots(&mut view, &[0, 1], n_heads));

		// after updating to a disjoint set,
		// the property that we are aware of all un-routed
		// from the perspective of our known leaves should
		// remain the same.

		view.update_leaves(
			&ctx,
			[hash(2), hash(3), hash(4)].iter().cloned(),
		).unwrap();

		assert!(check_roots(&mut view, &[2, 3, 4], n_heads));
	}

	#[test]
	fn update_leaves_overlapping() {
		let mut ctx = TestChainContext::default();
		let n_heads = 5;

		for i in 0..n_heads {
			ctx.known_map.insert(hash(i as u8), Known::Leaf);

			let messages_out: Vec<_> = (0..n_heads).filter_map(|j| message_queue_root(i, j)).collect();

			if !messages_out.is_empty() {
				ctx.ingress_roots.insert(hash(i as u8), messages_out);
			}
		}

		let mut view = View::default();
		view.update_leaves(
			&ctx,
			[hash(0), hash(1), hash(2)].iter().cloned(),
		).unwrap();

		assert!(check_roots(&mut view, &[0, 1, 2], n_heads));

		view.update_leaves(
			&ctx,
			[hash(2), hash(3), hash(4)].iter().cloned(),
		).unwrap();

		// after updating to a leaf-set overlapping with the prior,
		// the property that we are aware of all un-routed
		// from the perspective of our known leaves should
		// remain the same.

		assert!(check_roots(&mut view, &[2, 3, 4], n_heads));
	}
}
