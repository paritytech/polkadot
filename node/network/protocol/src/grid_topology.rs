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
use polkadot_primitives::v2::{AuthorityDiscoveryId, SessionIndex, ValidatorIndex};
use rand::{CryptoRng, Rng};
use std::{
	collections::{hash_map, HashMap, HashSet},
	fmt::Debug,
};

const LOG_TARGET: &str = "parachain::grid-topology";

/// The sample rate for randomly propagating messages. This
/// reduces the left tail of the binomial distribution but also
/// introduces a bias towards peers who we sample before others
/// (i.e. those who get a block before others).
pub const DEFAULT_RANDOM_SAMPLE_RATE: usize = crate::MIN_GOSSIP_PEERS;

/// The number of peers to randomly propagate messages to.
pub const DEFAULT_RANDOM_CIRCULATION: usize = 4;

/// Information about a peer in the gossip topology for a session.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyPeerInfo {
	/// The validator's known peer IDs.
	pub peer_ids: Vec<PeerId>,
	/// The index of the validator in the discovery keys of the corresponding
	/// `SessionInfo`. This can extend _beyond_ the set of active parachain validators.
	pub validator_index: ValidatorIndex,
	/// The authority discovery public key of the validator in the corresponding
	/// `SessionInfo`.
	pub discovery_id: AuthorityDiscoveryId,
}

/// Topology representation for a session.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SessionGridTopology {
	/// An array mapping validator indices to their indices in the
	/// shuffling itself. This has the same size as the number of validators
	/// in the session.
	shuffled_indices: Vec<usize>,
	/// The canonical shuffling of validators for the session.
	canonical_shuffling: Vec<TopologyPeerInfo>,
}

impl SessionGridTopology {
	/// Create a new session grid topology.
	pub fn new(shuffled_indices: Vec<usize>, canonical_shuffling: Vec<TopologyPeerInfo>) -> Self {
		SessionGridTopology { shuffled_indices, canonical_shuffling }
	}

	/// Produces the outgoing routing logic for a particular peer.
	///
	/// Returns `None` if the validator index is out of bounds.
	pub fn compute_grid_neighbors_for(&self, v: ValidatorIndex) -> Option<GridNeighbors> {
		if self.shuffled_indices.len() != self.canonical_shuffling.len() {
			return None
		}
		let shuffled_val_index = *self.shuffled_indices.get(v.0 as usize)?;

		let neighbors = matrix_neighbors(shuffled_val_index, self.shuffled_indices.len())?;

		let mut grid_subset = GridNeighbors::empty();
		for r_n in neighbors.row_neighbors {
			let n = &self.canonical_shuffling[r_n];
			grid_subset.validator_indices_x.insert(n.validator_index);
			for p in &n.peer_ids {
				grid_subset.peers_x.insert(*p);
			}
		}

		for c_n in neighbors.column_neighbors {
			let n = &self.canonical_shuffling[c_n];
			grid_subset.validator_indices_y.insert(n.validator_index);
			for p in &n.peer_ids {
				grid_subset.peers_y.insert(*p);
			}
		}

		Some(grid_subset)
	}
}

struct MatrixNeighbors<R, C> {
	row_neighbors: R,
	column_neighbors: C,
}

/// Compute the row and column neighbors of `val_index` in a matrix
fn matrix_neighbors(
	val_index: usize,
	len: usize,
) -> Option<MatrixNeighbors<impl Iterator<Item = usize>, impl Iterator<Item = usize>>> {
	if val_index >= len {
		return None
	}

	// e.g. for size 11 the matrix would be
	//
	// 0  1  2
	// 3  4  5
	// 6  7  8
	// 9 10
	//
	// and for index 10, the neighbors would be 1, 4, 7, 9

	let sqrt = (len as f64).sqrt() as usize;
	let our_row = val_index / sqrt;
	let our_column = val_index % sqrt;
	let row_neighbors = our_row * sqrt..std::cmp::min(our_row * sqrt + sqrt, len);
	let column_neighbors = (our_column..len).step_by(sqrt);

	Some(MatrixNeighbors {
		row_neighbors: row_neighbors.filter(move |i| *i != val_index),
		column_neighbors: column_neighbors.filter(move |i| *i != val_index),
	})
}

/// Information about the grid neighbors for a particular node in the topology.
#[derive(Debug, Clone, PartialEq)]
pub struct GridNeighbors {
	/// Represent peers in the X axis
	pub peers_x: HashSet<PeerId>,
	/// Represent validators in the X axis
	pub validator_indices_x: HashSet<ValidatorIndex>,
	/// Represent peers in the Y axis
	pub peers_y: HashSet<PeerId>,
	/// Represent validators in the Y axis
	pub validator_indices_y: HashSet<ValidatorIndex>,
}

impl GridNeighbors {
	/// Utility function for creating an empty set of grid neighbors.
	/// Useful for testing.
	pub fn empty() -> Self {
		GridNeighbors {
			peers_x: HashSet::new(),
			validator_indices_x: HashSet::new(),
			peers_y: HashSet::new(),
			validator_indices_y: HashSet::new(),
		}
	}

	/// Given the originator of a message as a validator index, indicates the part of the topology
	/// we're meant to send the message to.
	pub fn required_routing_by_index(
		&self,
		originator: ValidatorIndex,
		local: bool,
	) -> RequiredRouting {
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

	/// Given the originator of a message as a peer index, indicates the part of the topology
	/// we're meant to send the message to.
	pub fn required_routing_by_peer_id(&self, originator: PeerId, local: bool) -> RequiredRouting {
		if local {
			return RequiredRouting::GridXY
		}

		let grid_x = self.peers_x.contains(&originator);
		let grid_y = self.peers_y.contains(&originator);

		match (grid_x, grid_y) {
			(false, false) => RequiredRouting::None,
			(true, false) => RequiredRouting::GridY, // messages from X go to Y
			(false, true) => RequiredRouting::GridX, // messages from Y go to X
			(true, true) => {
				gum::debug!(
					target: LOG_TARGET,
					?originator,
					"Grid topology is unexpected, play it safe and send to X AND Y"
				);
				RequiredRouting::GridXY
			}, // if the grid works as expected, this shouldn't happen.
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
	pub fn peers_diff(&self, other: &Self) -> Vec<PeerId> {
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

/// An entry tracking a session grid topology and some cached local neighbors.
#[derive(Debug)]
pub struct SessionGridTopologyEntry {
	topology: SessionGridTopology,
	local_neighbors: GridNeighbors,
}

impl SessionGridTopologyEntry {
	/// Access the local grid neighbors.
	pub fn local_grid_neighbors(&self) -> &GridNeighbors {
		&self.local_neighbors
	}

	/// Access the local grid neighbors mutably.
	pub fn local_grid_neighbors_mut(&mut self) -> &mut GridNeighbors {
		&mut self.local_neighbors
	}

	/// Access the underlying topology.
	pub fn get(&self) -> &SessionGridTopology {
		&self.topology
	}
}

/// A set of topologies indexed by session
#[derive(Default)]
pub struct SessionGridTopologies {
	inner: HashMap<SessionIndex, (Option<SessionGridTopologyEntry>, usize)>,
}

impl SessionGridTopologies {
	/// Returns a topology for the specific session index
	pub fn get_topology(&self, session: SessionIndex) -> Option<&SessionGridTopologyEntry> {
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
	pub fn insert_topology(
		&mut self,
		session: SessionIndex,
		topology: SessionGridTopology,
		local_index: Option<ValidatorIndex>,
	) {
		let entry = self.inner.entry(session).or_insert((None, 0));
		if entry.0.is_none() {
			let local_neighbors = local_index
				.and_then(|l| topology.compute_grid_neighbors_for(l))
				.unwrap_or_else(GridNeighbors::empty);

			entry.0 = Some(SessionGridTopologyEntry { topology, local_neighbors });
		}
	}
}

/// A simple storage for a topology and the corresponding session index
#[derive(Debug)]
struct GridTopologySessionBound {
	entry: SessionGridTopologyEntry,
	session_index: SessionIndex,
}

/// A storage for the current and maybe previous topology
#[derive(Debug)]
pub struct SessionBoundGridTopologyStorage {
	current_topology: GridTopologySessionBound,
	prev_topology: Option<GridTopologySessionBound>,
}

impl Default for SessionBoundGridTopologyStorage {
	fn default() -> Self {
		// having this struct be `Default` is objectively stupid
		// but used in a few places
		SessionBoundGridTopologyStorage {
			current_topology: GridTopologySessionBound {
				// session 0 is valid so we should use the upper bound
				// as the default instead of the lower bound.
				session_index: SessionIndex::max_value(),
				entry: SessionGridTopologyEntry {
					topology: SessionGridTopology {
						shuffled_indices: Vec::new(),
						canonical_shuffling: Vec::new(),
					},
					local_neighbors: GridNeighbors::empty(),
				},
			},
			prev_topology: None,
		}
	}
}

impl SessionBoundGridTopologyStorage {
	/// Return a grid topology based on the session index:
	/// If we need a previous session and it is registered in the storage, then return that session.
	/// Otherwise, return a current session to have some grid topology in any case
	pub fn get_topology_or_fallback(&self, idx: SessionIndex) -> &SessionGridTopologyEntry {
		self.get_topology(idx).unwrap_or(&self.current_topology.entry)
	}

	/// Return the grid topology for the specific session index, if no such a session is stored
	/// returns `None`.
	pub fn get_topology(&self, idx: SessionIndex) -> Option<&SessionGridTopologyEntry> {
		if let Some(prev_topology) = &self.prev_topology {
			if idx == prev_topology.session_index {
				return Some(&prev_topology.entry)
			}
		}
		if self.current_topology.session_index == idx {
			return Some(&self.current_topology.entry)
		}

		None
	}

	/// Update the current topology preserving the previous one
	pub fn update_topology(
		&mut self,
		session_index: SessionIndex,
		topology: SessionGridTopology,
		local_index: Option<ValidatorIndex>,
	) {
		let local_neighbors = local_index
			.and_then(|l| topology.compute_grid_neighbors_for(l))
			.unwrap_or_else(GridNeighbors::empty);

		let old_current = std::mem::replace(
			&mut self.current_topology,
			GridTopologySessionBound {
				entry: SessionGridTopologyEntry { topology, local_neighbors },
				session_index,
			},
		);
		self.prev_topology.replace(old_current);
	}

	/// Returns a current grid topology
	pub fn get_current_topology(&self) -> &SessionGridTopologyEntry {
		&self.current_topology.entry
	}

	/// Access the current grid topology mutably. Dangerous and intended
	/// to be used in tests.
	pub fn get_current_topology_mut(&mut self) -> &mut SessionGridTopologyEntry {
		&mut self.current_topology.entry
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

	#[test]
	fn test_matrix_neighbors() {
		for (our_index, len, expected_row, expected_column) in vec![
			(0usize, 1usize, vec![], vec![]),
			(1, 2, vec![], vec![0usize]),
			(0, 9, vec![1, 2], vec![3, 6]),
			(9, 10, vec![], vec![0, 3, 6]),
			(10, 11, vec![9], vec![1, 4, 7]),
			(7, 11, vec![6, 8], vec![1, 4, 10]),
		]
		.into_iter()
		{
			let matrix = matrix_neighbors(our_index, len).unwrap();
			let mut row_result: Vec<_> = matrix.row_neighbors.collect();
			let mut column_result: Vec<_> = matrix.column_neighbors.collect();
			row_result.sort();
			column_result.sort();

			assert_eq!(row_result, expected_row);
			assert_eq!(column_result, expected_column);
		}
	}
}
