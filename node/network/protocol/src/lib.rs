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

#![deny(unused_crate_dependencies, unused_results)]
#![warn(missing_docs)]

use polkadot_primitives::v1::Hash;
use parity_scale_codec::{Encode, Decode};
use std::convert::TryFrom;
use std::fmt;

pub use sc_network::{ReputationChange, PeerId};

/// A unique identifier of a request.
pub type RequestId = u64;

/// A version of the protocol.
pub type ProtocolVersion = u32;

/// An error indicating that this the over-arching message type had the wrong variant
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrongVariant;

impl fmt::Display for WrongVariant {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "Wrong message variant")
	}
}

impl std::error::Error for WrongVariant {}


/// The peer-sets that the network manages. Different subsystems will use different peer-sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeerSet {
	/// The validation peer-set is responsible for all messages related to candidate validation and communication among validators.
	Validation,
	/// The collation peer-set is used for validator<>collator communication.
	Collation,
}

/// The advertised role of a node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObservedRole {
	/// A light node.
	Light,
	/// A full node.
	Full,
	/// A node claiming to be an authority (unauthenticated)
	Authority,
}

impl From<sc_network::ObservedRole> for ObservedRole {
	fn from(role: sc_network::ObservedRole) -> ObservedRole {
		match role {
			sc_network::ObservedRole::Light => ObservedRole::Light,
			sc_network::ObservedRole::Authority => ObservedRole::Authority,
			sc_network::ObservedRole::Full
				| sc_network::ObservedRole::OurSentry
				| sc_network::ObservedRole::OurGuardedAuthority
				=> ObservedRole::Full,
		}
	}
}

impl Into<sc_network::ObservedRole> for ObservedRole {
	fn into(self) -> sc_network::ObservedRole {
		match self {
			ObservedRole::Light => sc_network::ObservedRole::Light,
			ObservedRole::Full => sc_network::ObservedRole::Full,
			ObservedRole::Authority => sc_network::ObservedRole::Authority,
		}
	}
}

/// Events from network.
#[derive(Debug, Clone, PartialEq)]
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

macro_rules! impl_try_from {
	($m_ty:ident, $variant:ident, $out:ty) => {
		impl TryFrom<$m_ty> for $out {
			type Error = crate::WrongVariant;

			#[allow(unreachable_patterns)] // when there is only one variant
			fn try_from(x: $m_ty) -> Result<$out, Self::Error> {
				match x {
					$m_ty::$variant(y) => Ok(y),
					_ => Err(crate::WrongVariant),
				}
			}
		}

		impl<'a> TryFrom<&'a $m_ty> for &'a $out {
			type Error = crate::WrongVariant;

			fn try_from(x: &'a $m_ty) -> Result<&'a $out, Self::Error> {
				#[allow(unreachable_patterns)] // when there is only one variant
				match *x {
					$m_ty::$variant(ref y) => Ok(y),
					_ => Err(crate::WrongVariant),
				}
			}
		}
	}
}

impl<M> NetworkBridgeEvent<M> {
	/// Focus an overarching network-bridge event into some more specific variant.
	///
	/// This acts as a call to `clone`, except in the case where the event is a message event,
	/// in which case the clone can be expensive and it only clones if the message type can
	/// be focused.
	pub fn focus<'a, T>(&'a self) -> Result<NetworkBridgeEvent<T>, WrongVariant>
		where T: 'a + Clone, &'a T: TryFrom<&'a M, Error = WrongVariant>
	{
		Ok(match *self {
			NetworkBridgeEvent::PeerConnected(ref peer, ref role)
				=> NetworkBridgeEvent::PeerConnected(peer.clone(), role.clone()),
			NetworkBridgeEvent::PeerDisconnected(ref peer)
				=> NetworkBridgeEvent::PeerDisconnected(peer.clone()),
			NetworkBridgeEvent::PeerMessage(ref peer, ref msg)
				=> NetworkBridgeEvent::PeerMessage(peer.clone(), <&'a T>::try_from(msg)?.clone()),
			NetworkBridgeEvent::PeerViewChange(ref peer, ref view)
				=> NetworkBridgeEvent::PeerViewChange(peer.clone(), view.clone()),
			NetworkBridgeEvent::OurViewChange(ref view)
				=> NetworkBridgeEvent::OurViewChange(view.clone()),
		})
	}
}

/// A succinct representation of a peer's view. This consists of a bounded amount of chain heads.
///
/// Up to `N` (5?) chain heads.
#[derive(Default, Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct View(pub Vec<Hash>);

impl View {
	/// Replace `self` with `new`.
	///
	/// Returns an iterator that will yield all elements of `new` that were not part of `self`.
	pub fn replace_difference(&mut self, new: View) -> impl Iterator<Item = &Hash> {
		let old = std::mem::replace(self, new);

		self.0.iter().filter(move |h| !old.contains(h))
	}

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
		SignedAvailabilityBitfield, PoV, CandidateHash,
	};
	use polkadot_node_primitives::SignedFullStatement;
	use parity_scale_codec::{Encode, Decode};
	use std::convert::TryFrom;
	use super::RequestId;

	/// Network messages used by the availability distribution subsystem
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum AvailabilityDistributionMessage {
		/// An erasure chunk for a given candidate hash.
		#[codec(index = "0")]
		Chunk(CandidateHash, ErasureChunk),
	}

	/// Network messages used by the bitfield distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum BitfieldDistributionMessage {
		/// A signed availability bitfield for a given relay-parent hash.
		#[codec(index = "0")]
		Bitfield(Hash, SignedAvailabilityBitfield),
	}

	/// Network messages used by the PoV distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum PoVDistributionMessage {
		/// Notification that we are awaiting the given PoVs (by hash) against a
		/// specific relay-parent hash.
		#[codec(index = "0")]
		Awaiting(Hash, Vec<Hash>),
		/// Notification of an awaited PoV, in a given relay-parent context.
		/// (relay_parent, pov_hash, pov)
		#[codec(index = "1")]
		SendPoV(Hash, Hash, PoV),
	}

	/// Network messages used by the statement distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum StatementDistributionMessage {
		/// A signed full statement under a given relay-parent.
		#[codec(index = "0")]
		Statement(Hash, SignedFullStatement)
	}

	/// Network messages used by the collator protocol subsystem
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum CollatorProtocolMessage {
		/// Declare the intent to advertise collations under a collator ID.
		#[codec(index = "0")]
		Declare(CollatorId),
		/// Advertise a collation to a validator. Can only be sent once the peer has declared
		/// that they are a collator with given ID.
		#[codec(index = "1")]
		AdvertiseCollation(Hash, ParaId),
		/// Request the advertised collation at that relay-parent.
		#[codec(index = "2")]
		RequestCollation(RequestId, Hash, ParaId),
		/// A requested collation.
		#[codec(index = "3")]
		Collation(RequestId, CandidateReceipt, PoV),
	}

	/// All network messages on the validation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum ValidationProtocol {
		/// Availability distribution messages
		#[codec(index = "0")]
		AvailabilityDistribution(AvailabilityDistributionMessage),
		/// Bitfield distribution messages
		#[codec(index = "1")]
		BitfieldDistribution(BitfieldDistributionMessage),
		/// PoV Distribution messages
		#[codec(index = "2")]
		PoVDistribution(PoVDistributionMessage),
		/// Statement distribution messages
		#[codec(index = "3")]
		StatementDistribution(StatementDistributionMessage),
	}

	impl_try_from!(ValidationProtocol, AvailabilityDistribution, AvailabilityDistributionMessage);
	impl_try_from!(ValidationProtocol, BitfieldDistribution, BitfieldDistributionMessage);
	impl_try_from!(ValidationProtocol, PoVDistribution, PoVDistributionMessage);
	impl_try_from!(ValidationProtocol, StatementDistribution, StatementDistributionMessage);

	/// All network messages on the collation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub enum CollationProtocol {
		/// Collator protocol messages
		#[codec(index = "0")]
		CollatorProtocol(CollatorProtocolMessage),
	}

	impl_try_from!(CollationProtocol, CollatorProtocol, CollatorProtocolMessage);
}
