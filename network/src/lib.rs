// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! High-level network protocols for Polkadot.
//!
//! This manages routing for parachain statements, parachain block and outgoing message
//! data fetching, communication between collators and validators, and more.

#![recursion_limit="256"]

use polkadot_primitives::v0::{Block, Hash, BlakeTwo256, HashT};

pub mod legacy;
pub mod protocol;

/// Specialization of the network service for the polkadot block type.
pub type PolkadotNetworkService = sc_network::NetworkService<Block, Hash>;

mod cost {
	use sc_network::ReputationChange as Rep;
	pub(super) const UNEXPECTED_MESSAGE: Rep = Rep::new(-200, "Polkadot: Unexpected message");
	pub(super) const INVALID_FORMAT: Rep = Rep::new(-200, "Polkadot: Bad message");

	pub(super) const UNKNOWN_PEER: Rep = Rep::new(-50, "Polkadot: Unknown peer");
	pub(super) const BAD_COLLATION: Rep = Rep::new(-1000, "Polkadot: Bad collation");
}

mod benefit {
	use sc_network::ReputationChange as Rep;
	pub(super) const VALID_FORMAT: Rep = Rep::new(20, "Polkadot: Valid message format");
	pub(super) const GOOD_COLLATION: Rep = Rep::new(100, "Polkadot: Good collation");
}

/// Compute gossip topic for the erasure chunk messages given the hash of the
/// candidate they correspond to.
fn erasure_coding_topic(candidate_hash: &Hash) -> Hash {
	let mut v = candidate_hash.as_ref().to_vec();
	v.extend(b"erasure_chunks");

	BlakeTwo256::hash(&v[..])
}
