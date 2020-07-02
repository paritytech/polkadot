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

//! PoV Distribution Subsystem of Polkadot.
//!
//! This is a gossip implementation of code that is responsible for distributing PoVs
//! among validators.

use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{PoVBlock as PoV};
use polkadot_subsystem::{OverseerSignal, SubsystemContext, Subsystem, SubsystemResult};
use polkadot_subsystem::messages::{
	PoVDistributionMessage, NetworkBridgeEvent, ObservedRole, ReputationChange as Rep, PeerId,
};

use parity_scale_codec::{Encode, Decode};

#[derive(Encode, Decode)]
enum NetworkMessage {
    /// Notification that we are awaiting the given PoVs (by hash) against a
	/// specific relay-parent hash.
	#[codec(index = "0")]
    Awaiting(Hash, Vec<Hash>),
    /// Notification of an awaited PoV, in a given relay-parent context.
    /// (relay_parent, pov_hash, pov)
	#[codec(index = "1")]
    SendPoV(Hash, Hash, PoV),
}
