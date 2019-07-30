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

use sr_primitives::traits::{BlakeTwo256, Hash as HashT};
use polkadot_primitives::Hash;
use std::collections::{HashMap, HashSet};
use substrate_client::error::Error as ClientError;
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
	expected_queues: HashMap<Hash, Hash>, // topic -> queue-root
}

impl View {
	/// Update the set of current leaves.
	pub fn update_leaves<T: ChainContext + ?Sized, I>(&mut self, context: &T, new_leaves: I)
		-> Result<(), ClientError>
		where I: Iterator<Item=Hash>
	{
		let new_leaves = new_leaves.take(MAX_CHAIN_HEADS);
		let old_leaves = {
			let mut new = LeavesVec::new();
			for leaf in new_leaves {
				new.push(leaf.clone());
			}

			std::mem::replace(&mut self.leaves, new)
		};

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
				expected_queues.insert(topic, queue_root);
			});

			if r.is_err() {
				res = r;
			}

			self.leaf_topics.insert(*new_leaf, this_leaf_topics);
		}

		res
	}

	/// Validate an incoming message queue against this view.
	pub fn validate_queue(&self, messages: &super::GossipParachainMessages)
		-> (GossipValidationResult<Hash>, i32)
	{
		let ostensible_topic = queue_topic(messages.queue_root);
		if !self.is_topic_live(&ostensible_topic) {
			(GossipValidationResult::Discard, super::cost::UNNEEDED_ICMP_MESSAGES)
		} else if !messages.queue_root_is_correct() {
			(
				GossipValidationResult::Discard,
				super::cost::icmp_messages_root_mismatch(messages.messages.len()),
			)
		} else {
			(
				GossipValidationResult::ProcessAndKeep(ostensible_topic),
				super::benefit::NEW_ICMP_MESSAGES,
			)
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

	fn check_roots(view: &View, i: u8, max: u8) -> bool {
		for j in 0..max {
			if let Some(messages) = message_queue(i, j) {
				let queue_root = message_queue_root(i, j).unwrap();
				let messages = GossipParachainMessages {
					queue_root,
					messages: messages.iter().map(|m| ParachainMessage(m.to_vec())).collect(),
				};

				match view.validate_queue(&messages).0 {
					GossipValidationResult::ProcessAndKeep(topic) => if topic != queue_topic(queue_root) {
						return false
					},
					_ => return false,
				}
			}
		}

		true
	}

	#[test]
	fn update_leaves_none_in_common() {
		let mut ctx = TestChainContext::default();
		let max = 5;

		for i in 0..max {
			ctx.known_map.insert(hash(i as u8), Known::Leaf);

			let messages_out: Vec<_> = (0..max).filter_map(|j| message_queue_root(i, j)).collect();

			if !messages_out.is_empty() {
				ctx.ingress_roots.insert(hash(i as u8), messages_out);
			}
		}

		let mut view = View::default();
		view.update_leaves(
			&ctx,
			[hash(0), hash(1)].iter().cloned(),
		).unwrap();

		assert!(check_roots(&view, 0, max));
		assert!(check_roots(&view, 1, max));

		assert!(!check_roots(&view, 2, max));
		assert!(!check_roots(&view, 3, max));
		assert!(!check_roots(&view, 4, max));
		assert!(!check_roots(&view, 5, max));

		view.update_leaves(
			&ctx,
			[hash(2), hash(3), hash(4)].iter().cloned(),
		).unwrap();

		assert!(!check_roots(&view, 0, max));
		assert!(!check_roots(&view, 1, max));

		assert!(check_roots(&view, 2, max));
		assert!(check_roots(&view, 3, max));
		assert!(check_roots(&view, 4, max));

		assert!(!check_roots(&view, 5, max));
	}

	#[test]
	fn update_leaves_overlapping() {
		let mut ctx = TestChainContext::default();
		let max = 5;

		for i in 0..max {
			ctx.known_map.insert(hash(i as u8), Known::Leaf);

			let messages_out: Vec<_> = (0..max).filter_map(|j| message_queue_root(i, j)).collect();

			if !messages_out.is_empty() {
				ctx.ingress_roots.insert(hash(i as u8), messages_out);
			}
		}

		let mut view = View::default();
		view.update_leaves(
			&ctx,
			[hash(0), hash(1), hash(2)].iter().cloned(),
		).unwrap();

		view.update_leaves(
			&ctx,
			[hash(2), hash(3), hash(4)].iter().cloned(),
		).unwrap();

		assert!(!check_roots(&view, 0, max));
		assert!(!check_roots(&view, 1, max));

		assert!(check_roots(&view, 2, max));
		assert!(check_roots(&view, 3, max));
		assert!(check_roots(&view, 4, max));

		assert!(!check_roots(&view, 5, max));
	}
}
