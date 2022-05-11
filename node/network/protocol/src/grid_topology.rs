// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Grid topology support implementation
//! The basic operation of the 2D grid topology is that:
//!   * A validator producing a message sends it to its row-neighbors and its column-neighbors
//!   * A validator receiving a message originating from one of its row-neighbors sends it to its column-neighbors
//!   * A validator receiving a message originating from one of its column-neighbors sends it to its row-neighbors
//!
//! This grid approach defines 2 unique paths for every validator to reach every other validator in at most 2 hops.
//!
//! However, we also supplement this with some degree of random propagation:
//! every validator, upon seeing a message for the first time, propagates it to 8 random peers.
//! This inserts some redundancy in case the grid topology isn't working or is being attacked -
//! an adversary doesn't know which peers a validator will send to.
//! This is combined with the property that the adversary doesn't know which validators will elect to check a block.
//!

use crate::PeerId;
use polkadot_primitives::v2::{SessionIndex, ValidatorIndex};
use rand::{CryptoRng, Rng};
use std::{
	collections::{hash_map, HashMap, HashSet},
	fmt::Debug,
};

/// The sample rate for randomly propagating messages. This
/// reduces the left tail of the binomial distribution but also
/// introduces a bias towards peers who we sample before others
/// (i.e. those who get a block before others).
pub const DEFAULT_RANDOM_SAMPLE_RATE: usize = crate::MIN_GOSSIP_PEERS;

/// The number of peers to randomly propagate messages to.
pub const DEFAULT_RANDOM_CIRCULATION: usize = 4;

/// Topology representation
#[derive(Default, Clone, Debug)]
pub struct SessionGridTopology {
	/// Represent peers in the X axis
	pub peers_x: HashSet<PeerId>,
	/// Represent validators in the X axis
	pub validator_indices_x: HashSet<ValidatorIndex>,
	/// Represent peers in the Y axis
	pub peers_y: HashSet<PeerId>,
	/// Represent validators in the Y axis
	pub validator_indices_y: HashSet<ValidatorIndex>,
}

impl SessionGridTopology {
	/// Given the originator of a message, indicates the part of the topology
	/// we're meant to send the message to.
	pub fn required_routing_for(&self, originator: ValidatorIndex, local: bool) -> RequiredRouting {
		if local {
			return RequiredRouting::GridXY
		}

		let grid_x = self.validator_indices_x.contains(&originator);
		let grid_y = self.validator_indices_y.contains(&originator);

		match (grid_x, grid_y) {
			(false, false) => RequiredRouting::None,
			(true, false) => RequiredRouting::GridY, // messages from X go to Y
			(false, true) => RequiredRouting::GridX, // messages from Y go to X
			(true, true) => RequiredRouting::GridXY, // if the grid works as expected, this shouldn't happen.
		}
	}

	/// Get a filter function based on this topology and the required routing
	/// which returns `true` for peers that are within the required routing set
	/// and false otherwise.
	pub fn route_to_peer(&self, required_routing: RequiredRouting, peer: &PeerId) -> bool {
		match required_routing {
			RequiredRouting::All => true,
			RequiredRouting::GridX => self.peers_x.contains(peer),
			RequiredRouting::GridY => self.peers_y.contains(peer),
			RequiredRouting::GridXY => self.peers_x.contains(peer) || self.peers_y.contains(peer),
			RequiredRouting::None | RequiredRouting::PendingTopology => false,
		}
	}

	/// Returns the difference between this and the `other` topology as a vector of peers
	pub fn peers_diff(&self, other: &SessionGridTopology) -> Vec<PeerId> {
		self.peers_x
			.iter()
			.chain(self.peers_y.iter())
			.filter(|peer_id| !(other.peers_x.contains(peer_id) || other.peers_y.contains(peer_id)))
			.cloned()
			.collect::<Vec<_>>()
	}

	/// A convenience method that returns total number of peers in the topology
	pub fn len(&self) -> usize {
		self.peers_x.len().saturating_add(self.peers_y.len())
	}
}

/// A set of topologies indexed by session
#[derive(Default)]
pub struct SessionGridTopologies {
	inner: HashMap<SessionIndex, (Option<SessionGridTopology>, usize)>,
}

impl SessionGridTopologies {
	/// Returns a topology for the specific session index
	pub fn get_topology(&self, session: SessionIndex) -> Option<&SessionGridTopology> {
		self.inner.get(&session).and_then(|val| val.0.as_ref())
	}

	/// Increase references counter for a specific topology
	pub fn inc_session_refs(&mut self, session: SessionIndex) {
		self.inner.entry(session).or_insert((None, 0)).1 += 1;
	}

	/// Decrease references counter for a specific topology
	pub fn dec_session_refs(&mut self, session: SessionIndex) {
		if let hash_map::Entry::Occupied(mut occupied) = self.inner.entry(session) {
			occupied.get_mut().1 = occupied.get().1.saturating_sub(1);
			if occupied.get().1 == 0 {
				let _ = occupied.remove();
			}
		}
	}

	/// Insert a new topology, no-op if already present.
	pub fn insert_topology(&mut self, session: SessionIndex, topology: SessionGridTopology) {
		let entry = self.inner.entry(session).or_insert((None, 0));
		if entry.0.is_none() {
			entry.0 = Some(topology);
		}
	}
}
/// A representation of routing based on sample
#[derive(Debug, Clone, Copy)]
pub struct RandomRouting {
	/// The number of peers to target.
	target: usize,
	/// The number of peers this has been sent to.
	sent: usize,
	/// Sampling rate
	sample_rate: usize,
}

impl Default for RandomRouting {
	fn default() -> Self {
		RandomRouting {
			target: DEFAULT_RANDOM_CIRCULATION,
			sent: 0_usize,
			sample_rate: DEFAULT_RANDOM_SAMPLE_RATE,
		}
	}
}

impl RandomRouting {
	/// Perform random sampling for a specific peer
	/// Returns `true` for a lucky peer
	pub fn sample(&self, n_peers_total: usize, rng: &mut (impl CryptoRng + Rng)) -> bool {
		if n_peers_total == 0 || self.sent >= self.target {
			false
		} else if self.sample_rate > n_peers_total {
			true
		} else {
			rng.gen_ratio(self.sample_rate as _, n_peers_total as _)
		}
	}

	/// Increase number of messages being sent
	pub fn inc_sent(&mut self) {
		self.sent += 1
	}
}

/// Routing mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RequiredRouting {
	/// We don't know yet, because we're waiting for topology info
	/// (race condition between learning about the first blocks in a new session
	/// and getting the topology for that session)
	PendingTopology,
	/// Propagate to all peers of any kind.
	All,
	/// Propagate to all peers sharing either the X or Y dimension of the grid.
	GridXY,
	/// Propagate to all peers sharing the X dimension of the grid.
	GridX,
	/// Propagate to all peers sharing the Y dimension of the grid.
	GridY,
	/// No required propagation.
	None,
}

impl RequiredRouting {
	/// Whether the required routing set is definitely empty.
	pub fn is_empty(self) -> bool {
		match self {
			RequiredRouting::PendingTopology | RequiredRouting::None => true,
			_ => false,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rand::SeedableRng;
	use rand_chacha::ChaCha12Rng;

	fn dummy_rng() -> ChaCha12Rng {
		rand_chacha::ChaCha12Rng::seed_from_u64(12345)
	}

	#[test]
	fn test_random_routing_sample() {
		// This test is fragile as it relies on a specific ChaCha12Rng
		// sequence that might be implementation defined even for a static seed
		let mut rng = dummy_rng();
		let mut random_routing = RandomRouting { target: 4, sent: 0, sample_rate: 8 };

		assert_eq!(random_routing.sample(16, &mut rng), true);
		random_routing.inc_sent();
		assert_eq!(random_routing.sample(16, &mut rng), false);
		assert_eq!(random_routing.sample(16, &mut rng), false);
		assert_eq!(random_routing.sample(16, &mut rng), true);
		random_routing.inc_sent();
		assert_eq!(random_routing.sample(16, &mut rng), true);
		random_routing.inc_sent();
		assert_eq!(random_routing.sample(16, &mut rng), false);
		assert_eq!(random_routing.sample(16, &mut rng), false);
		assert_eq!(random_routing.sample(16, &mut rng), false);
		assert_eq!(random_routing.sample(16, &mut rng), true);
		random_routing.inc_sent();

		for _ in 0..16 {
			assert_eq!(random_routing.sample(16, &mut rng), false);
		}
	}

	fn run_random_routing(
		random_routing: &mut RandomRouting,
		rng: &mut (impl CryptoRng + Rng),
		npeers: usize,
		iters: usize,
	) -> usize {
		let mut ret = 0_usize;

		for _ in 0..iters {
			if random_routing.sample(npeers, rng) {
				random_routing.inc_sent();
				ret += 1;
			}
		}

		ret
	}

	#[test]
	fn test_random_routing_distribution() {
		let mut rng = dummy_rng();

		let mut random_routing = RandomRouting { target: 4, sent: 0, sample_rate: 8 };
		assert_eq!(run_random_routing(&mut random_routing, &mut rng, 100, 10000), 4);

		let mut random_routing = RandomRouting { target: 8, sent: 0, sample_rate: 100 };
		assert_eq!(run_random_routing(&mut random_routing, &mut rng, 100, 10000), 8);

		let mut random_routing = RandomRouting { target: 0, sent: 0, sample_rate: 100 };
		assert_eq!(run_random_routing(&mut random_routing, &mut rng, 100, 10000), 0);

		let mut random_routing = RandomRouting { target: 10, sent: 0, sample_rate: 10 };
		assert_eq!(run_random_routing(&mut random_routing, &mut rng, 10, 100), 10);
	}
}
