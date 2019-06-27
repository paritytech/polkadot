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

use sr_primitives::traits::{ProvideRuntimeApi, BlakeTwo256, Hash as HashT};
use sr_primitives::generic::BlockId;
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::ParachainHost;
use std::collections::{HashMap, HashSet};
use super::{MAX_CHAIN_HEADS, LeavesVec};

/// Construct a topic for a message queue root deterministically.
pub fn queue_topic(queue_root: Hash) -> Hash {
	let mut v = queue_root.as_ref().to_vec();
	v.extend(b"message_queue");

	BlakeTwo256::hash(&v[..])
}

/// A type-safe handle to a leaf.
pub trait Leaf {
	fn hash(&self) -> &Hash;
}

impl Leaf for Hash {
	fn hash(&self) -> &Hash {
		self
	}
}

/// Context to the underlying polkadot chain.
pub trait ChainContext {
	/// The leaf type that this chain context is associated with.
	type Leaf: Leaf;
	/// An error that can occur when querying the chain.
	type Error;

	/// Provide a closure which is invoked for every unrouted queue hash at a given leaf.
	fn leaf_unrouted_roots<F: FnMut(&Hash)>(&self, leaf: &Self::Leaf, with_queue_root: F)
		-> Result<(), Self::Error>;
}

impl<P: ProvideRuntimeApi> ChainContext for P where P::Api: ParachainHost<Block> {
	type Leaf = Hash;
	type Error = substrate_client::error::Error;

	fn leaf_unrouted_roots<F: FnMut(&Hash)>(&self, &leaf: &Self::Leaf, mut with_queue_root: F)
		-> Result<(), Self::Error>
	{
		let api = self.runtime_api();

		let leaf_id = BlockId::Hash(leaf);
		let active_parachains = api.active_parachains(&leaf_id)?;

		for para_id in active_parachains {
			if let Some(ingress) = api.ingress(&leaf_id, para_id)? {
				for (_height, _from, queue_root) in ingress.iter() {
					with_queue_root(queue_root);
				}
			}
		}

		Ok(())
	}
}

/// A view of which queue roots are current for a given set of leaves.
pub struct View {
	leaves: LeavesVec,
	leaf_topics: HashMap<Hash, HashSet<Hash>>, // leaf_hash -> { topics }
	expected_queues: HashMap<Hash, Hash>, // topic -> queue-root
}

impl View {
	/// Update the set of current leaves.
	pub fn update_leaves<T: ChainContext>(&mut self, context: &T, new_leaves: &[T::Leaf])
		-> Result<(), T::Error>
	{
		let new_leaves = new_leaves.iter().take(MAX_CHAIN_HEADS);
		let old_leaves = {
			let mut new = LeavesVec::new();
			for leaf in new_leaves.clone() {
				new.push(leaf.hash().clone());
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

		// add in new data about fresh leaves.
		for new_leaf in new_leaves {
			let hash = new_leaf.hash();
			if old_leaves.contains(hash) { continue }

			let mut this_leaf_topics = HashSet::new();
			context.leaf_unrouted_roots(new_leaf, |&queue_root| {
				let topic = queue_topic(queue_root);
				this_leaf_topics.insert(topic);
				expected_queues.insert(topic, queue_root);
			})?;

			self.leaf_topics.insert(*hash, this_leaf_topics);
		}

		Ok(())
	}

	/// Whether a message with given topic is expired.
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

	/// Whether a message is allowed based on our leaf-set.
	pub fn allowed(&self, topic: &Hash) -> bool {
		self.expected_queues.contains_key(topic)
	}
}
