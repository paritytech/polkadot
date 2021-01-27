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

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use polkadot_primitives::v1::{Hash, BlockNumber};
use parity_scale_codec::{Encode, Decode};
use std::{convert::TryFrom, fmt, collections::HashMap};

pub use sc_network::{ReputationChange, PeerId};
#[doc(hidden)]
pub use polkadot_node_jaeger::JaegerSpan;
#[doc(hidden)]
pub use std::sync::Arc;


/// Peer-sets and protocols used for parachains.
pub mod peer_set;

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

	/// Our view has changed.
	OurViewChange(OurView),
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

/// Specialized wrapper around [`View`].
///
/// Besides the access to the view itself, it also gives access to the [`JaegerSpan`] per leave/head.
#[derive(Debug, Clone, Default)]
pub struct OurView {
	view: View,
	span_per_head: HashMap<Hash, Arc<JaegerSpan>>,
}

impl OurView {
	/// Creates a new instance.
	pub fn new(heads: impl IntoIterator<Item = (Hash, Arc<JaegerSpan>)>, finalized_number: BlockNumber) -> Self {
		let state_per_head = heads.into_iter().collect::<HashMap<_, _>>();

		Self {
			view: View {
				heads: state_per_head.keys().cloned().collect(),
				finalized_number,
			},
			span_per_head: state_per_head,
		}
	}

	/// Returns the span per head map.
	///
	/// For each head there exists one span in this map.
	pub fn span_per_head(&self) -> &HashMap<Hash, Arc<JaegerSpan>> {
		&self.span_per_head
	}
}

impl PartialEq for OurView {
	fn eq(&self, other: &Self) -> bool {
		self.view == other.view
	}
}

impl std::ops::Deref for OurView {
	type Target = View;

	fn deref(&self) -> &View {
		&self.view
	}
}

/// Construct a new [`OurView`] with the given chain heads, finalized number 0 and disabled [`JaegerSpan`]'s.
///
/// NOTE: Use for tests only.
///
/// # Example
///
/// ```
/// # use polkadot_node_network_protocol::our_view;
/// # use polkadot_primitives::v1::Hash;
/// let our_view = our_view![Hash::repeat_byte(1), Hash::repeat_byte(2)];
/// ```
#[macro_export]
macro_rules! our_view {
	( $( $hash:expr ),* $(,)? ) => {
		$crate::OurView::new(
			vec![ $( $hash.clone() ),* ].into_iter().map(|h| (h, $crate::Arc::new($crate::JaegerSpan::Disabled))),
			0,
		)
	};
}

/// A succinct representation of a peer's view. This consists of a bounded amount of chain heads
/// and the highest known finalized block number.
///
/// Up to `N` (5?) chain heads.
#[derive(Default, Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct View {
	/// A bounded amount of chain heads.
	pub heads: Vec<Hash>,
	/// The highest known finalized block number.
	pub finalized_number: BlockNumber,
}

/// Construct a new view with the given chain heads and finalized number 0.
///
/// NOTE: Use for tests only.
///
/// # Example
///
/// ```
/// # use polkadot_node_network_protocol::view;
/// # use polkadot_primitives::v1::Hash;
/// let view = view![Hash::repeat_byte(1), Hash::repeat_byte(2)];
/// ```
#[macro_export]
macro_rules! view {
	( $( $hash:expr ),* $(,)? ) => {
		$crate::View { heads: vec![ $( $hash.clone() ),* ], finalized_number: 0 }
	};
}

impl View {
	/// Replace `self` with `new`.
	///
	/// Returns an iterator that will yield all elements of `new` that were not part of `self`.
	pub fn replace_difference(&mut self, new: View) -> impl Iterator<Item = &Hash> {
		let old = std::mem::replace(self, new);

		self.heads.iter().filter(move |h| !old.contains(h))
	}

	/// Returns an iterator of the hashes present in `Self` but not in `other`.
	pub fn difference<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.heads.iter().filter(move |h| !other.contains(h))
	}

	/// An iterator containing hashes present in both `Self` and in `other`.
	pub fn intersection<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.heads.iter().filter(move |h| other.contains(h))
	}

	/// Whether the view contains a given hash.
	pub fn contains(&self, hash: &Hash) -> bool {
		self.heads.contains(hash)
	}
}

/// v1 protocol types.
pub mod v1 {
	use polkadot_primitives::v1::{
		Hash, CollatorId, Id as ParaId, ErasureChunk, CandidateReceipt,
		SignedAvailabilityBitfield, PoV, CandidateHash, ValidatorIndex, CandidateIndex,
	};
	use polkadot_node_primitives::{
		SignedFullStatement,
		approval::{IndirectAssignmentCert, IndirectSignedApprovalVote},
	};
	use parity_scale_codec::{Encode, Decode};
	use super::RequestId;
	use std::convert::TryFrom;

	/// Network messages used by the availability distribution subsystem
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum AvailabilityDistributionMessage {
		/// An erasure chunk for a given candidate hash.
		#[codec(index = "0")]
		Chunk(CandidateHash, ErasureChunk),
	}

	/// Network messages used by the availability recovery subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum AvailabilityRecoveryMessage {
		/// Request a chunk for a given candidate hash and validator index.
		RequestChunk(RequestId, CandidateHash, ValidatorIndex),
		/// Respond with chunk for a given candidate hash and validator index.
		/// The response may be `None` if the requestee does not have the chunk.
		Chunk(RequestId, Option<ErasureChunk>),
	}

	/// Network messages used by the bitfield distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum BitfieldDistributionMessage {
		/// A signed availability bitfield for a given relay-parent hash.
		#[codec(index = "0")]
		Bitfield(Hash, SignedAvailabilityBitfield),
	}

	/// Network messages used by the PoV distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum PoVDistributionMessage {
		/// Notification that we are awaiting the given PoVs (by hash) against a
		/// specific relay-parent hash.
		#[codec(index = "0")]
		Awaiting(Hash, Vec<Hash>),
		/// Notification of an awaited PoV, in a given relay-parent context.
		/// (relay_parent, pov_hash, compressed_pov)
		#[codec(index = "1")]
		SendPoV(Hash, Hash, CompressedPoV),
	}

	/// Network messages used by the statement distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum StatementDistributionMessage {
		/// A signed full statement under a given relay-parent.
		#[codec(index = "0")]
		Statement(Hash, SignedFullStatement)
	}

	/// Network messages used by the approval distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum ApprovalDistributionMessage {
		/// Assignments for candidates in recent, unfinalized blocks.
		///
		/// Actually checking the assignment may yield a different result.
		#[codec(index = "0")]
		Assignments(Vec<(IndirectAssignmentCert, CandidateIndex)>),
		/// Approvals for candidates in some recent, unfinalized block.
		#[codec(index = "1")]
		Approvals(Vec<IndirectSignedApprovalVote>),
	}

	#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
	#[allow(missing_docs)]
	pub enum CompressedPoVError {
		#[error("Failed to compress a PoV")]
		Compress,
		#[error("Failed to decompress a PoV")]
		Decompress,
		#[error("Failed to decode the uncompressed PoV")]
		Decode,
		#[error("Architecture is not supported")]
		NotSupported,
	}

	/// SCALE and Zstd encoded [`PoV`].
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct CompressedPoV(Vec<u8>);

	impl CompressedPoV {
		/// Compress the given [`PoV`] and returns a [`CompressedPoV`].
		#[cfg(not(target_os = "unknown"))]
		pub fn compress(pov: &PoV) -> Result<Self, CompressedPoVError> {
			zstd::encode_all(pov.encode().as_slice(), 3).map_err(|_| CompressedPoVError::Compress).map(Self)
		}

		/// Compress the given [`PoV`] and returns a [`CompressedPoV`].
		#[cfg(target_os = "unknown")]
		pub fn compress(_: &PoV) -> Result<Self, CompressedPoVError> {
			Err(CompressedPoVError::NotSupported)
		}

		/// Decompress `self` and returns the [`PoV`] on success.
		#[cfg(not(target_os = "unknown"))]
		pub fn decompress(&self) -> Result<PoV, CompressedPoVError> {
			use std::io::Read;
			const MAX_POV_BLOCK_SIZE: usize = 32 * 1024 * 1024;

			struct InputDecoder<'a, T: std::io::BufRead>(&'a mut zstd::Decoder<T>, usize);
			impl<'a, T: std::io::BufRead> parity_scale_codec::Input for InputDecoder<'a, T> {
				fn read(&mut self, into: &mut [u8]) -> Result<(), parity_scale_codec::Error> {
					self.1 = self.1.saturating_add(into.len());
					if self.1 > MAX_POV_BLOCK_SIZE {
						return Err("pov block too big".into())
					}
					self.0.read_exact(into).map_err(Into::into)
				}
				fn remaining_len(&mut self) -> Result<Option<usize>, parity_scale_codec::Error> {
					Ok(None)
				}
			}

			let mut decoder = zstd::Decoder::new(self.0.as_slice()).map_err(|_| CompressedPoVError::Decompress)?;
			PoV::decode(&mut InputDecoder(&mut decoder, 0)).map_err(|_| CompressedPoVError::Decode)
		}

		/// Decompress `self` and returns the [`PoV`] on success.
		#[cfg(target_os = "unknown")]
		pub fn decompress(&self) -> Result<PoV, CompressedPoVError> {
			Err(CompressedPoVError::NotSupported)
		}
	}

	/// Network messages used by the collator protocol subsystem
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
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
		Collation(RequestId, CandidateReceipt, CompressedPoV),
	}

	/// All network messages on the validation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
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
		/// Availability recovery messages
		#[codec(index = "4")]
		AvailabilityRecovery(AvailabilityRecoveryMessage),
		/// Approval distribution messages
		#[codec(index = "5")]
		ApprovalDistribution(ApprovalDistributionMessage),
	}

	impl_try_from!(ValidationProtocol, AvailabilityDistribution, AvailabilityDistributionMessage);
	impl_try_from!(ValidationProtocol, BitfieldDistribution, BitfieldDistributionMessage);
	impl_try_from!(ValidationProtocol, PoVDistribution, PoVDistributionMessage);
	impl_try_from!(ValidationProtocol, StatementDistribution, StatementDistributionMessage);
	impl_try_from!(ValidationProtocol, ApprovalDistribution, ApprovalDistributionMessage);

	/// All network messages on the collation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum CollationProtocol {
		/// Collator protocol messages
		#[codec(index = "0")]
		CollatorProtocol(CollatorProtocolMessage),
	}

	impl_try_from!(CollationProtocol, CollatorProtocol, CollatorProtocolMessage);
}

#[cfg(test)]
mod tests {
	use polkadot_primitives::v1::PoV;
	use super::v1::{CompressedPoV, CompressedPoVError};

	#[test]
	fn decompress_huge_pov_block_fails() {
		let pov = PoV { block_data: vec![0; 63 * 1024 * 1024].into() };

		let compressed = CompressedPoV::compress(&pov).unwrap();
		assert_eq!(CompressedPoVError::Decode, compressed.decompress().unwrap_err());
	}
}
