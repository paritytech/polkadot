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

use polkadot_primitives::{Block, Hash};

pub mod legacy;
pub mod protocol;

sc_network::construct_simple_protocol! {
	/// Stub until https://github.com/paritytech/substrate/pull/4665 is merged
	pub struct PolkadotProtocol where Block = Block { }
}

/// Specialization of the network service for the polkadot protocol.
pub type PolkadotNetworkService = sc_network::NetworkService<Block, PolkadotProtocol, Hash>;

mod cost {
	use sc_network::ReputationChange as Rep;
	pub(super) const UNEXPECTED_MESSAGE: Rep = Rep::new(-200, "Polkadot: Unexpected message");
	pub(super) const UNEXPECTED_ROLE: Rep = Rep::new(-200, "Polkadot: Unexpected role");
	pub(super) const INVALID_FORMAT: Rep = Rep::new(-200, "Polkadot: Bad message");

	pub(super) const UNKNOWN_PEER: Rep = Rep::new(-50, "Polkadot: Unknown peer");
	pub(super) const COLLATOR_ALREADY_KNOWN: Rep = Rep::new(-100, "Polkadot: Known collator");
	pub(super) const BAD_COLLATION: Rep = Rep::new(-1000, "Polkadot: Bad collation");
	pub(super) const BAD_POV_BLOCK: Rep = Rep::new(-1000, "Polkadot: Bad POV block");
}

mod benefit {
	use sc_network::ReputationChange as Rep;
	pub(super) const EXPECTED_MESSAGE: Rep = Rep::new(20, "Polkadot: Expected message");
	pub(super) const VALID_FORMAT: Rep = Rep::new(20, "Polkadot: Valid message format");

	pub(super) const KNOWN_PEER: Rep = Rep::new(5, "Polkadot: Known peer");
	pub(super) const NEW_COLLATOR: Rep = Rep::new(10, "Polkadot: New collator");
	pub(super) const GOOD_COLLATION: Rep = Rep::new(100, "Polkadot: Good collation");
	pub(super) const GOOD_POV_BLOCK: Rep = Rep::new(100, "Polkadot: Good POV block");
}

