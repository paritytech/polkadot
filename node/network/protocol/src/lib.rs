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

//! Network protocol types for parachains.

use polkadot_primitives::v1::Hash;
use parity_scale_codec::{Encode, Decode};

pub use sc_network::{ObservedRole, ReputationChange, PeerId};

/// A unique identifier of a request.
pub type RequestId = u64;

/// A version of the protocol.
pub type ProtocolVersion = u32;

/// The peer-sets that the network manages. Different subsystems will use different peer-sets.
#[derive(Debug, Clone, Copy)]
pub enum PeerSet {
	/// The validation peer-set is responsible for all messages related to candidate validation and communication among validators.
	Validation,
	/// The collation peer-set is used for validator<>collator communication.
	Collation,
}

/// Events from network.
#[derive(Debug, Clone)]
pub enum NetworkBridgeEvent<M> {
	/// A peer has connected.
	PeerConnected(PeerId, ObservedRole),

	/// A peer has disconnected.
	PeerDisconnected(PeerId),

	/// Peer has sent a message.
	PeerMessage(PeerId, M),

	/// Peer's `View` has changed.
	PeerViewChange(PeerId, View),

	/// Our `View` has changed.
	OurViewChange(View),
}


/// A succinct representation of a peer's view. This consists of a bounded amount of chain heads.
///
/// Up to `N` (5?) chain heads.
#[derive(Default, Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct View(pub Vec<Hash>);

impl View {
	/// Returns an iterator of the hashes present in `Self` but not in `other`.
	pub fn difference<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.0.iter().filter(move |h| !other.contains(h))
	}

	/// An iterator containing hashes present in both `Self` and in `other`.
	pub fn intersection<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.0.iter().filter(move |h| other.contains(h))
	}

	/// Whether the view contains a given hash.
	pub fn contains(&self, hash: &Hash) -> bool {
		self.0.contains(hash)
	}
}


/// v1 protocol types.
pub mod v1 {
	use polkadot_primitives::v1::{
		Hash, CollatorId, Id as ParaId, ErasureChunk, CandidateReceipt,
		SignedAvailabilityBitfield, PoV,
	};
	use polkadot_node_primitives::SignedFullStatement;
	use parity_scale_codec::{Encode, Decode};
	use super::RequestId;

	/// Network messages used by the availability distribution subsystem
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum AvailabilityDistributionMessage {
		/// An erasure chunk for a given candidate hash.
		Chunk(Hash, ErasureChunk),
	}

	/// Network messages used by the bitfield distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum BitfieldDistributionMessage {
		/// A signed availability bitfield for a given relay-parent hash.
		Bitfield(Hash, SignedAvailabilityBitfield),
	}

	/// Network messages used by the PoV distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum PoVDistributionMessage {
		/// Notification that we are awaiting the given PoVs (by hash) against a
		/// specific relay-parent hash.
		Awaiting(Hash, Vec<Hash>),
		/// Notification of an awaited PoV, in a given relay-parent context.
		/// (relay_parent, pov_hash, pov)
		SendPoV(Hash, Hash, PoV),
	}

	/// Network messages used by the statement distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum StatementDistributionMessage {
		/// A signed full statement under a given relay-parent.
		Statement(Hash, SignedFullStatement)
	}

	/// Network messages used by the collator protocol subsystem
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum CollatorProtocolMessage {
		/// Declare the intent to advertise collations under a collator ID.
		Declare(CollatorId),
		/// Advertise a collation to a validator. Can only be sent once the peer has declared
		/// that they are a collator with given ID.
		AdvertiseCollation(Hash, ParaId),
		/// Request the advertised collation at that relay-parent.
		RequestCollation(RequestId, Hash, ParaId),
		/// A requested collation.
		Collation(RequestId, CandidateReceipt, PoV),
	}

	/// All network messages on the validation peer-set.
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum ValidationProtocol {
		/// Availability distribution messages
		AvailabilityDistribution(AvailabilityDistributionMessage),
		/// Bitfield distribution messages
		BitfieldDistribution(BitfieldDistributionMessage),
		/// PoV Distribution messages
		PoVDistribution(PoVDistributionMessage),
		/// Statement distribution messages
		StatementDistribution(StatementDistributionMessage),
	}

	/// All network messages on the collation peer-set.
	#[derive(Debug, Clone, Encode, Decode)]
	pub enum CollationProtocol {
		/// Collator protocol messages
		CollatorProtocol(CollatorProtocolMessage),
	}
}
